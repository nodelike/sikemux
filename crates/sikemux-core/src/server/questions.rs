//! What `ask` answers, and the CLIs' own question tools with it. An agent's
//! question goes first to the agent that started it, which decides. A lead that
//! ends its turn without answering leaves an assumption standing, and is handed
//! a question the asker waits on back once before it reaches the person. Only
//! what a lead raises itself, or what an agent with no lead asks, reaches them.
//! The person answers in the asker's chat, the notch or Sikemux's rail, and the
//! first answer reaches the asker as a message.

use crate::server::changed::What;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use crate::cli::protocol::HarnessRequest;
use crate::text::clipped;
use sikemux_view::inbox::{InboxRaised, RaisedFor};
use sikemux_view::questions::{AgentQuestion, QuestionChoice, QuestionState};
use sikemux_wire::protocol::ChatContext;
use sikemux_wire::protocol::WindowCall;

use super::activity::Activity;
use super::approvals::{Approval, Keeper};
use super::changed::Topic;
use super::chat::ChatCommand;
use super::connection::ClientId;
use super::kept::Kept;
use super::messages;
use super::notify::Signal;
use super::wake::{sooner, Wake};
use super::Core;

mod relay;
mod reply;

use relay::*;
pub(crate) use relay::{answer_relayed, closed_elsewhere, relayed_here};
use reply::*;

const FILE: &str = "agent-questions.json";
/// Where the person's answers to secret questions wait for their agents.
const SECRETS: &str = "agent-secrets";
/// The answers to secret questions kept for one agent, newest first.
const MAX_SECRETS_PER_AGENT: usize = 8;
const MAX_QUESTION_BYTES: usize = 8 * 1024;
const MAX_CHOICES: usize = 9;
const MAX_LABEL_CHARS: usize = 200;
const MAX_DETAIL_CHARS: usize = 400;
/// A question's title on the tab the person picks it by.
const MAX_HEADER_CHARS: usize = 40;
const MAX_ANSWER_BYTES: usize = 64 * 1024;
const MAX_OPEN_PER_AGENT: usize = 8;
/// Answers kept to show while their askers have not read them; the oldest go first.
const MAX_ANSWERED: usize = 32;
/// A lead whose turn ended may still be handed the question back unread, so
/// it counts as passing it over only once this long has gone by.
const LEAD_GRACE_MS: u64 = 3_000;
/// The fewest words that can say what is being decided without the chat around them.
const MIN_RAISE_WORDS: usize = 5;
const WINDOW_ANSWER_WAIT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "stage")]
enum Stage {
    /// Sent to the agent that started the asker. `ended_at` is when that
    /// agent's turn ended after it was handed the question, and `handed_back`
    /// that it has had the question a second time.
    #[serde(rename_all = "camelCase")]
    Lead {
        lead_id: String,
        lead_name: String,
        ended_at: Option<u64>,
        #[serde(default)]
        handed_back: bool,
    },
    /// Put to the person. `lead` is the agent it went to first, which
    /// raised itself, which `passed` says, rather than leaving it unanswered.
    #[serde(rename_all = "camelCase")]
    Person {
        lead: Option<(String, String)>,
        passed: bool,
    },
}

/// What makes a decision put to the person stand alone, beside its question and choices.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Raise {
    reason: RaisedFor,
    /// The choice the lead would take.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pick: Option<String>,
    /// What breaks if that is wrong, or what stays stuck without the access.
    breaks: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Question {
    id: String,
    agent_id: String,
    name: String,
    cli: String,
    project: String,
    /// The asker's own words, which its chat shows the question under.
    asked: String,
    /// What the person is asked, which a lead that passes it on may reword.
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    header: Option<String>,
    choices: Vec<QuestionChoice>,
    /// The person may pick more than one of `choices`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    multi: bool,
    blocked: bool,
    assumption: Option<String>,
    /// The answer is a secret, such as a password, so the question goes
    /// straight to the person and the answer never enters a transcript.
    #[serde(default)]
    secret: bool,
    at: u64,
    stage: Stage,
    /// The linked host it was put to the person on, where its asker's team
    /// is led from.
    #[serde(skip_serializing_if = "Option::is_none")]
    relayed: Option<String>,
    /// The linked host its asker runs on, when it is put to the person here
    /// for an agent there.
    #[serde(skip_serializing_if = "Option::is_none")]
    from_host: Option<String>,
    /// The plugin tool call the person approves or turns down with their
    /// answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    approval: Option<Approval>,
    /// The dialog in the asker's process that waits on the answer, which then
    /// goes to it rather than arriving as a message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dialog: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    raise: Option<Raise>,
}

impl Question {
    fn view(&self) -> AgentQuestion {
        let (state, via_note) = match &self.stage {
            Stage::Lead { .. } => (QuestionState::Lead, None),
            Stage::Person { lead, passed } => (
                if self.blocked {
                    QuestionState::Open
                } else {
                    QuestionState::Assumed
                },
                lead.as_ref().map(|(_, name)| {
                    let why = if *passed {
                        "raised this"
                    } else {
                        "didn't answer"
                    };
                    format!("@{name} {why}")
                }),
            ),
        };
        let text = match &self.raise {
            Some(raise) if raise.reason == RaisedFor::Access => {
                format!("{}\n\nWithout it: {}", self.text, raise.breaks)
            }
            Some(raise) => format!("{}\n\nIf that pick is wrong: {}", self.text, raise.breaks),
            None => self.text.clone(),
        };
        AgentQuestion {
            id: self.id.clone(),
            agent_id: self.agent_id.clone(),
            name: self.name.clone(),
            cli: self.cli.clone(),
            project: self.project.clone(),
            asked: self.asked.clone(),
            text,
            header: self.header.clone(),
            choices: self.choices.clone(),
            multi: self.multi,
            blocked: self.blocked,
            assumption: self.assumption.clone(),
            secret: self.secret,
            at: self.at,
            state,
            via_note,
            secret_note: self.secret.then(|| format!(
                "Kept private. {} reads it from a file only your account can open, and it never shows in the chat.",
                self.name
            )),
            answer: None,
            ticket: None,
        }
    }

    fn asks_person(&self) -> bool {
        matches!(self.stage, Stage::Person { .. }) && self.relayed.is_none() && !self.stands()
    }

    /// An assumption nobody raised needs no answer: it goes on the record as decided. One an
    /// agent with no lead to ask went on with still reaches the person when it is theirs to
    /// decide.
    fn stands(&self) -> bool {
        let unled = matches!(self.stage, Stage::Person { lead: None, .. });
        matches!(self.stage, Stage::Person { .. })
            && self.assumption.is_some()
            && self.raise.is_none()
            && !(unled && self.persons_call())
            && self.from_host.is_none()
    }

    /// Whether what it asks is the person's to decide: it offers them choices, or names a push,
    /// a merge or a deploy.
    fn persons_call(&self) -> bool {
        let lower = self.text.to_lowercase();
        let ships = (lower.split(|c: char| !c.is_alphanumeric())).any(|word| {
            ["push", "merg", "deploy"]
                .iter()
                .any(|stem| word.starts_with(stem))
        });
        !self.choices.is_empty() || ships
    }

    fn lead_id(&self) -> Option<&str> {
        match &self.stage {
            Stage::Lead { lead_id, .. } => Some(lead_id),
            Stage::Person { .. } => None,
        }
    }

    /// Whether `lead` was handed the question, before or after it went on
    /// to the person.
    fn went_to(&self, lead: &str) -> bool {
        match &self.stage {
            Stage::Lead { lead_id, .. } => lead_id == lead,
            Stage::Person {
                lead: Some((lead_id, _)),
                ..
            } => lead_id == lead,
            Stage::Person { lead: None, .. } => false,
        }
    }
}

/// The questions still open, kept in the data folder.
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QuestionsRecord {
    open: Vec<Question>,
}

/// A question the person answered, kept until its asker reads the answer.
#[derive(Clone)]
struct Answered {
    question: Question,
    /// What it was answered with; none for a secret.
    answer: Option<String>,
}

/// One thing an agent asked the person in the reply its turn ended with, rather than with a
/// tool; a reply that asks several things makes one of these for each. An offer a yes or a no
/// answers is not one: it shows as how the agent's turn left things. Nothing holds the agent on
/// them, and its next turn takes them back.
#[derive(Clone)]
struct ReplyQuestion {
    id: String,
    agent_id: String,
    name: String,
    cli: String,
    project: String,
    text: String,
    /// The options of a choice the reply leaves to the person, the agent's pick marked.
    choices: Vec<QuestionChoice>,
    /// The person's answer, held until every question of the same reply has one, so the
    /// agent hears them all in one message.
    given: Option<String>,
    at: u64,
}

impl ReplyQuestion {
    fn waits(&self) -> bool {
        self.given.is_none()
    }

    fn view(&self) -> AgentQuestion {
        AgentQuestion {
            id: self.id.clone(),
            agent_id: self.agent_id.clone(),
            name: self.name.clone(),
            cli: self.cli.clone(),
            project: self.project.clone(),
            asked: self.text.clone(),
            text: self.text.clone(),
            header: None,
            choices: self.choices.clone(),
            multi: false,
            blocked: false,
            assumption: None,
            secret: false,
            at: self.at,
            state: QuestionState::Open,
            via_note: None,
            secret_note: None,
            answer: None,
            ticket: None,
        }
    }

    fn answered(&self) -> messages::Answered {
        messages::Answered {
            question: self.text.clone(),
            asker: self.name.clone(),
            asker_id: self.agent_id.clone(),
            cli: self.cli.clone(),
            secret: false,
        }
    }
}

#[derive(Default)]
struct State {
    open: Vec<Question>,
    /// The questions of each agent's latest reply, in the order it asked them.
    reply_questions: Vec<ReplyQuestion>,
    /// The rule each agent was last sent back to over what it offered, so the same rule does not
    /// send it back again until a turn ends offering nothing a rule is about.
    followed_rules: HashMap<String, String>,
    /// The agents whose latest turn ended in questions they were sent back to, so the next such
    /// turn reaches the person.
    sent_back: HashSet<String>,
    /// Oldest first.
    answered: Vec<Answered>,
    /// Something changed that the window has not been sent yet.
    dirty: bool,
    /// The window connection last sent the questions.
    told_window: Option<ClientId>,
    /// The person's open questions phones were told of, and whose they are.
    told_phones: HashMap<String, String>,
    /// Every question put to the person, assumptions too, that devices'
    /// views last showed, and whose they are.
    shown_devices: HashMap<String, String>,
    keeper: Keeper,
}

pub(crate) struct Questions {
    state: Mutex<State>,
    kept: Kept,
    secrets: Option<std::path::PathBuf>,
    /// Moves questions on and tells the window and phones of them.
    pub(crate) wake: Wake,
}

#[derive(Clone)]
pub(crate) struct ForPerson {
    pub id: String,
    pub agent_id: String,
    pub text: String,
    pub blocked: bool,
    pub assumption: Option<String>,
    /// Only the person has what it asks for, such as a password or a sign-in.
    pub access: bool,
    /// The answers it can be given in one tap, in order.
    pub answers: Vec<String>,
    pub at: u64,
    pub raised: Option<InboxRaised>,
    /// The one of `answers` its asker would take.
    pub pick: Option<String>,
}

fn recommended_of(choices: &[QuestionChoice]) -> Option<String> {
    (choices.iter())
        .find(|choice| choice.recommended)
        .map(|choice| choice.label.clone())
}

struct Asked {
    text: String,
    header: Option<String>,
    choices: Vec<QuestionChoice>,
    multi: bool,
    blocked: bool,
    assumption: Option<String>,
    secret: bool,
    approval: Option<Approval>,
    dialog: Option<String>,
    raise: Option<Raise>,
}

impl Questions {
    /// Takes up the questions left open by agents `known` says still exist.
    pub(crate) fn new(data_dir: Option<&Path>, known: impl Fn(&str) -> bool) -> Self {
        let kept = Kept::new(data_dir, FILE);
        let mut record: QuestionsRecord = kept.load();
        let mut keeper = Keeper::new(data_dir.is_some());
        record.open.retain_mut(|question| {
            let known = known(&question.agent_id);
            match &mut question.approval {
                Some(approval) if known => keeper.restore(&question.id, approval),
                Some(_) => {
                    keeper.forget(&question.id);
                    false
                }
                None => known,
            }
        });
        let secrets = data_dir.map(|dir| dir.join(SECRETS));
        if let Some(entries) = secrets
            .as_deref()
            .and_then(|dir| std::fs::read_dir(dir).ok())
        {
            for entry in entries.flatten() {
                if !known(&entry.file_name().to_string_lossy()) {
                    let _ = std::fs::remove_dir_all(entry.path());
                }
            }
        }
        Self {
            secrets,
            state: Mutex::new(State {
                dirty: !record.open.is_empty(),
                open: record.open,
                keeper,
                ..State::default()
            }),
            kept,
            wake: Wake::default(),
        }
    }

    /// Keeps the person's answer to a secret question in a file only their
    /// account reads, keeping each agent's newest few, and answers its path.
    fn keep_secret(&self, question: &Question, answer: &str) -> Result<std::path::PathBuf, String> {
        let agent = Path::new(&question.agent_id);
        let folder = self
            .secrets
            .as_ref()
            .filter(|_| agent.file_name() == Some(agent.as_os_str()))
            .ok_or("Sikemux has nowhere private to keep a secret")?
            .join(agent);
        let path = folder.join(&question.id);
        sikemux_process::files::write_atomic(&path, answer.as_bytes(), true)
            .map_err(|error| format!("Sikemux could not keep the secret: {error}"))?;
        let mut kept: Vec<(std::time::SystemTime, std::path::PathBuf)> = std::fs::read_dir(&folder)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
            .collect();
        kept.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
        for (_, old) in kept.iter().skip(MAX_SECRETS_PER_AGENT) {
            let _ = std::fs::remove_file(old);
        }
        Ok(path)
    }

    /// The open questions changed: the window is told, and they are kept.
    fn changed(&self, state: &mut State) {
        self.kept.save(&QuestionsRecord {
            open: state.open.clone(),
        });
        let waiting = state
            .open
            .iter()
            .filter_map(|question| Some((question.id.as_str(), question.approval.as_ref()?)));
        state.keeper.keep(waiting);
        self.shown_changed(state);
    }

    /// What the window and devices show changed, though nothing kept did.
    fn shown_changed(&self, state: &mut State) {
        state.dirty = true;
        self.wake.now();
    }

    pub(crate) fn blocks_on_person(&self, agent_id: &str) -> bool {
        self.state.locked().open.iter().any(|question| {
            question.agent_id == agent_id && question.blocked && question.asks_person()
        })
    }

    /// The question the agent stopped for that waits on the person, by id.
    pub(crate) fn blocking(&self, agent_id: &str) -> Option<String> {
        (self.state.locked().open.iter())
            .find(|question| {
                question.agent_id == agent_id && question.blocked && question.asks_person()
            })
            .map(|question| question.id.clone())
    }

    /// Whether a question the agent asked, with the question tool or in its last reply, waits
    /// on the person.
    pub(crate) fn asks_person(&self, agent_id: &str) -> bool {
        self.blocks_on_person(agent_id)
            || (self.state.locked().reply_questions.iter())
                .any(|question| question.agent_id == agent_id && question.waits())
    }

    /// Whether the agent stopped to wait for the answer to a question.
    pub(crate) fn waits_on_answer(&self, agent_id: &str) -> bool {
        self.state
            .locked()
            .open
            .iter()
            .any(|question| question.agent_id == agent_id && question.blocked)
    }

    /// Whether the agent has a question or an assumption the person has not
    /// answered.
    pub(crate) fn has_open(&self, agent_id: &str) -> bool {
        self.state
            .locked()
            .open
            .iter()
            .any(|question| question.agent_id == agent_id)
    }

    /// The questions put to the person, with a tool or in a reply, oldest first: of those an
    /// agent waits on, only its latest.
    pub(crate) fn for_person(&self) -> Vec<ForPerson> {
        let state = self.state.locked();
        let asked = (state.open.iter())
            .filter(|question| question.asks_person())
            .map(|question| ForPerson {
                id: question.id.clone(),
                agent_id: question.agent_id.clone(),
                text: question.text.clone(),
                blocked: question.blocked,
                assumption: question.assumption.clone(),
                access: question.secret
                    || (question.raise.as_ref())
                        .is_some_and(|raise| raise.reason == RaisedFor::Access),
                answers: match question.multi {
                    true => Vec::new(),
                    false => (question.choices.iter())
                        .map(|choice| choice.label.clone())
                        .collect(),
                },
                at: question.at,
                raised: question.raise.as_ref().map(|raise| InboxRaised {
                    reason: raise.reason,
                    by: match &question.stage {
                        Stage::Person {
                            lead: Some((_, name)),
                            passed: true,
                        } => Some(name.clone()),
                        _ => None,
                    },
                    pick: raise.pick.clone(),
                    breaks: raise.breaks.clone(),
                }),
                pick: recommended_of(&question.choices),
            });
        let replied = (state.reply_questions.iter())
            .filter(|question| question.waits())
            .map(|question| ForPerson {
                id: question.id.clone(),
                agent_id: question.agent_id.clone(),
                text: question.text.clone(),
                blocked: true,
                assumption: None,
                access: false,
                answers: (question.choices.iter())
                    .map(|choice| choice.label.clone())
                    .collect(),
                at: question.at,
                raised: None,
                pick: recommended_of(&question.choices),
            });
        let questions: Vec<ForPerson> = asked.chain(replied).collect();
        let waits = |question: &ForPerson| question.assumption.is_none() || question.blocked;
        let mut latest: HashMap<&str, (u64, &str)> = HashMap::new();
        for question in questions.iter().filter(|question| waits(question)) {
            let kept = latest.entry(&question.agent_id).or_insert((0, ""));
            if kept.1.is_empty() || kept.0 < question.at {
                *kept = (question.at, &question.id);
            }
        }
        let shown: HashSet<String> = latest.values().map(|(_, id)| (*id).to_owned()).collect();
        (questions.iter())
            .filter(|question| !waits(question) || shown.contains(&question.id))
            .cloned()
            .collect()
    }

    pub(crate) fn asks_in_reply(&self, agent_id: &str) -> bool {
        (self.state.locked().reply_questions.iter()).any(|question| question.agent_id == agent_id)
    }

    /// The agent began another turn, as when the person wrote to it, so what it waited
    /// on the person to answer no longer waits. An assumption it went on with, and a plugin
    /// call waiting for the person's approval, still do.
    pub(crate) fn turn_started(&self, agent_id: &str) {
        let mut state = self.state.locked();
        let (replies, open) = (state.reply_questions.len(), state.open.len());
        state
            .reply_questions
            .retain(|question| question.agent_id != agent_id);
        state.open.retain(|question| {
            let waited_on = question.blocked && question.asks_person();
            question.agent_id != agent_id || !waited_on || question.approval.is_some()
        });
        if state.open.len() != open {
            self.changed(&mut state);
        } else if state.reply_questions.len() != replies {
            self.shown_changed(&mut state);
        }
    }

    /// The boards changed, which may change the ticket a question names.
    pub(crate) fn boards_changed(&self) {
        let mut state = self.state.locked();
        if !state.open.is_empty() {
            state.dirty = true;
            self.wake.now();
        }
    }

    /// The question `id` that `agent_id` asked and `lead` was handed, in the
    /// asker's own words, while it is open.
    pub(crate) fn asked_of(&self, lead: &str, agent_id: &str, id: &str) -> Option<String> {
        self.state
            .locked()
            .open
            .iter()
            .find(|question| {
                question.id == id && question.agent_id == agent_id && question.went_to(lead)
            })
            .map(|question| question.asked.clone())
    }

    /// Closes the question `id` its agent asked `lead`, which answered it, while it is open.
    fn take_answered(&self, lead: &str, agent_id: &str, id: &str) -> Option<Question> {
        let mut state = self.state.locked();
        let at = (state.open.iter()).position(|question| {
            question.id == id && question.agent_id == agent_id && question.went_to(lead)
        })?;
        let question = state.open.remove(at);
        self.changed(&mut state);
        Some(question)
    }

    /// The agent was handed the person's answers to its questions.
    pub(crate) fn delivered(&self, agent_id: &str) {
        let mut state = self.state.locked();
        let before = state.answered.len();
        state
            .answered
            .retain(|answered| answered.question.agent_id != agent_id);
        if state.answered.len() != before {
            self.changed(&mut state);
        }
    }

    /// The person typed into the terminal of an agent that asked them
    /// something, which answers it there.
    pub(crate) fn typed(&self, agent_id: &str) {
        let mut state = self.state.locked();
        let before = state.open.len();
        state
            .open
            .retain(|question| !(question.agent_id == agent_id && question.asks_person()));
        if state.open.len() != before {
            self.changed(&mut state);
        }
    }
}

/// The lead answered the question `id` its agent asked it, which settles it: the answer goes on
/// the record, with who decided and why.
pub(crate) fn lead_answered(
    core: &Core,
    lead: &str,
    agent_id: &str,
    id: &str,
    answer: &str,
    why: Option<String>,
) {
    let Some(question) = core.harness.questions.take_answered(lead, agent_id, id) else {
        return;
    };
    super::board::goal::keep_settled(
        core,
        super::board::goal::Settled {
            project: question.project,
            by: lead.to_owned(),
            holder: question.agent_id,
            asked: question.asked,
            text: answer.to_owned(),
            why,
        },
    );
}

/// An assumption stands: its asker goes on with it, and it goes on the record, where the person
/// can overrule it.
fn stood(core: &Core, question: Question) {
    let Some(assumption) = question.assumption else {
        return;
    };
    let why = match &question.stage {
        Stage::Person {
            lead: Some((_, lead)),
            ..
        } => format!(
            "@{lead} did not answer, so @{}'s assumption stands.",
            question.name
        ),
        _ => format!("@{} had no lead to ask.", question.name),
    };
    super::board::goal::keep_settled(
        core,
        super::board::goal::Settled {
            project: question.project,
            by: question.agent_id.clone(),
            holder: question.agent_id,
            asked: question.asked,
            text: assumption,
            why: Some(why),
        },
    );
}

/// The agent's turn ended, so a question it was handed and did not answer
/// starts its time to be passed over.
pub(crate) fn turn_ended(core: &Core, agent_id: &str) {
    let questions = &core.harness.questions;
    let waiting: Vec<String> = questions
        .state
        .locked()
        .open
        .iter()
        .filter(|question| question.lead_id() == Some(agent_id))
        .map(|question| question.id.clone())
        .collect();
    let read: HashSet<String> = waiting
        .into_iter()
        .filter(|id| !messages::is_waiting(core, agent_id, id))
        .collect();
    if read.is_empty() {
        return;
    }
    let now = epoch_ms();
    let mut state = questions.state.locked();
    for question in &mut state.open {
        if let Stage::Lead { ended_at, .. } = &mut question.stage {
            if read.contains(&question.id) && ended_at.is_none() {
                *ended_at = Some(now);
            }
        }
    }
    questions.changed(&mut state);
}

fn text_param(params: &Value, key: &str, max: usize) -> Result<Option<String>, String> {
    Ok(super::caller::text_param(params, key, max)?.map(|text| text.trim().to_owned()))
}

fn choices(value: Option<&Value>) -> Result<Vec<QuestionChoice>, String> {
    let list = match value {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(list)) => list,
        Some(_) => return Err("choices must be a list".into()),
    };
    if list.len() > MAX_CHOICES {
        return Err(format!("choices holds at most {MAX_CHOICES}"));
    }
    list.iter()
        .map(|choice| {
            let (label, detail) = match choice {
                Value::String(label) => (label.as_str(), None),
                Value::Object(choice) => (
                    choice
                        .get("label")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                    choice
                        .get("detail")
                        .or_else(|| choice.get("description"))
                        .and_then(Value::as_str),
                ),
                _ => ("", None),
            };
            let (label, recommended) = recommended(label);
            if label.is_empty() {
                return Err("each choice needs a label".to_owned());
            }
            Ok(QuestionChoice {
                label: clipped(label, MAX_LABEL_CHARS),
                detail: detail
                    .filter(|detail| !detail.trim().is_empty())
                    .map(|detail| clipped(detail, MAX_DETAIL_CHARS)),
                recommended,
            })
        })
        .collect()
}

/// A choice's label without the "(recommended)" the agent put after it, and whether it did.
fn recommended(label: &str) -> (&str, bool) {
    const MARK: &str = "(recommended)";
    let label = label.trim();
    let cut = label
        .len()
        .checked_sub(MARK.len())
        .filter(|&at| label.is_char_boundary(at));
    match cut {
        Some(at) if label[at..].eq_ignore_ascii_case(MARK) => (label[..at].trim_end(), true),
        _ => (label, false),
    }
}

/// What `ask` was given to raise a decision to the person, refused when it does not stand alone,
/// saying everything missing. The choice picked gets marked as recommended.
fn raise_of(
    params: &Value,
    question: &str,
    choices: &mut [QuestionChoice],
) -> Result<Raise, String> {
    const REASONS: &str = "irreversible, money, taste or access";
    let reason = match text_param(params, "reason", 40)?.as_deref() {
        Some("irreversible") => Some(RaisedFor::Irreversible),
        Some("money") => Some(RaisedFor::Money),
        Some("taste") => Some(RaisedFor::Taste),
        Some("access") => Some(RaisedFor::Access),
        Some(other) => {
            return Err(format!(
                "Only {REASONS} reach the person, not {other}: anything else is yours to decide, so answer it yourself"
            ))
        }
        None => None,
    };
    let needs_choice = reason != Some(RaisedFor::Access);
    mark_pick(params, choices)?;
    let pick = (choices.iter())
        .find(|choice| choice.recommended)
        .map(|choice| choice.label.clone());
    let breaks = text_param(params, "breaks", MAX_DETAIL_CHARS)?;
    let mut missing = Vec::new();
    if question.split_whitespace().count() < MIN_RAISE_WORDS {
        missing.push(format!(
            "question: what is being decided, in at least {MIN_RAISE_WORDS} words that make sense without this chat"
        ));
    }
    if reason.is_none() {
        missing.push(format!("reason: {REASONS}"));
    }
    if needs_choice && choices.len() < 2 {
        missing.push("choices: at least two options".to_owned());
    }
    if needs_choice && pick.is_none() {
        missing.push("pick: the choice you would take".to_owned());
    }
    if breaks.is_none() {
        missing.push("breaks: what breaks if that is wrong".to_owned());
    }
    match (reason, breaks) {
        (Some(reason), Some(breaks)) if missing.is_empty() => Ok(Raise {
            reason,
            pick,
            breaks,
        }),
        _ => Err(format!(
            "A decision raised to the person has to stand alone, and this one is missing:\n- {}\nIf none of {REASONS} is the reason, decide it yourself.",
            missing.join("\n- ")
        )),
    }
}

/// Marks the choice `ask` was given as `pick` as the one recommended.
fn mark_pick(params: &Value, choices: &mut [QuestionChoice]) -> Result<(), String> {
    let Some(pick) = text_param(params, "pick", MAX_LABEL_CHARS)? else {
        return Ok(());
    };
    let Some(at) = (choices.iter()).position(|choice| choice.label.eq_ignore_ascii_case(&pick))
    else {
        return Err(format!("pick must be one of the choices, not {pick}"));
    };
    for (index, choice) in choices.iter_mut().enumerate() {
        choice.recommended = index == at;
    }
    Ok(())
}

/// Whether the person closed the agent, so its questions go with it.
fn gone(core: &Core, agent_id: &str) -> bool {
    !core.agent_now(agent_id).running() && core.workspaces.closed(agent_id)
}

/// Whether a message reaches the agent now: it runs, or it sleeps in an open
/// window that wakes it. The window may not have said yet which chats it holds.
fn reachable(core: &Core, agent_id: &str) -> bool {
    core.agent_now(agent_id).running()
        || (core.window.is_open()
            && !core.workspaces.closed(agent_id)
            && core.workspaces.published(agent_id) != Some(false))
}

fn cli_of(core: &Core, agent_id: &str) -> String {
    core.agent_now(agent_id)
        .cli()
        .unwrap_or_else(|| "agent".to_owned())
}

/// The agent that started `agent_id`, while a message reaches it.
fn reachable_lead(core: &Core, agent_id: &str) -> Option<String> {
    let led_by_auto = (core.hosts.team.tree(agent_id)).is_some_and(|tree| tree.auto);
    (core.names.get(agent_id)?.parent).filter(|lead| !led_by_auto && reachable(core, lead))
}

/// Whether the agent answers to nobody but has agents of its own, so what it asks the person is
/// a decision it raises rather than a question from a lone agent.
fn raises(core: &Core, agent_id: &str) -> bool {
    reachable_lead(core, agent_id).is_none() && core.names.team(agent_id).len() > 1
}

/// Opens the questions `agent_id` asked together, put to its lead when it
/// has one a message reaches, else to the person. Opens none when they would
/// leave the agent with too many waiting.
fn open(
    core: &Core,
    agent_id: &str,
    project: &str,
    asked: Vec<Asked>,
) -> Result<Vec<Question>, String> {
    let named = core.names.get(agent_id);
    let stage = match reachable_lead(core, agent_id) {
        Some(lead_id) => Stage::Lead {
            lead_name: core.names.name_or_id(&lead_id),
            lead_id,
            ended_at: None,
            handed_back: false,
        },
        None => Stage::Person {
            lead: None,
            passed: false,
        },
    };
    let name = named
        .as_ref()
        .map(|named| named.name.clone())
        .unwrap_or_else(|| agent_id.to_owned());
    let project = named
        .map(|named| named.project)
        .unwrap_or_else(|| core.workspaces.project_of(std::path::Path::new(project)));
    let cli = cli_of(core, agent_id);
    let at = epoch_ms();
    let questions: Vec<Question> = asked
        .into_iter()
        .map(|asked| Question {
            stage: if asked.secret || asked.approval.is_some() || asked.dialog.is_some() {
                Stage::Person {
                    lead: None,
                    passed: false,
                }
            } else {
                stage.clone()
            },
            secret: asked.secret,
            id: format!("question-{}", uuid::Uuid::new_v4().simple()),
            agent_id: agent_id.to_owned(),
            name: name.clone(),
            cli: cli.clone(),
            project: project.clone(),
            asked: asked.text.clone(),
            text: asked.text,
            header: asked.header,
            multi: asked.multi && !asked.choices.is_empty(),
            choices: asked.choices,
            blocked: asked.blocked,
            assumption: asked.assumption,
            at,
            relayed: None,
            from_host: None,
            approval: asked.approval,
            dialog: asked.dialog,
            raise: asked.raise,
        })
        .collect();
    let mut state = core.harness.questions.state.locked();
    let open = state
        .open
        .iter()
        .filter(|open| open.agent_id == agent_id)
        .count();
    if open + questions.len() > MAX_OPEN_PER_AGENT {
        return Err(format!(
            "You would have more than {MAX_OPEN_PER_AGENT} questions waiting for answers; wait for those you asked first"
        ));
    }
    state.open.extend(questions.iter().cloned());
    core.harness.questions.changed(&mut state);
    Ok(questions)
}

fn moves_on(question: &Question) -> String {
    match &question.assumption {
        Some(assumption) => {
            format!("Go on assuming {assumption}. A message tells you if that is overruled.")
        }
        None => {
            "Stop here and end your turn without guessing: the answer comes to you as a message."
                .to_owned()
        }
    }
}

/// What the asker is told about where its question went.
fn sent_note(question: &Question) -> String {
    let to = match &question.stage {
        Stage::Lead { lead_name, .. } => {
            format!("Sent to @{lead_name}, your lead, which decides it.")
        }
        Stage::Person { .. } if question.stands() => {
            "Nobody has to answer this: it is on the record as your decision, where the person can overrule it.".to_owned()
        }
        Stage::Person { .. } => "Sent to the person.".to_owned(),
    };
    format!("{to} {}", moves_on(question))
}

/// The question as the lead reads it.
fn for_lead(question: &Question) -> (String, String) {
    let name = &question.name;
    let mut text = question.text.clone();
    if !question.choices.is_empty() {
        text.push_str("\n\nChoices:");
        for (index, choice) in question.choices.iter().enumerate() {
            text.push_str(&format!("\n{}. {}", index + 1, choice.label));
            if let Some(detail) = &choice.detail {
                text.push_str(&format!(" — {detail}"));
            }
        }
    }
    match &question.assumption {
        Some(assumption) => text.push_str(&format!(
            "\n\n@{name} goes on assuming {assumption} until it hears otherwise."
        )),
        None => text.push_str(&format!(
            "\n\n@{name} waits for the answer before it goes on."
        )),
    }
    let if_silent = match &question.assumption {
        Some(_) => "its assumption stands and goes on the record".to_owned(),
        None => "you get it once more to decide, then it goes to the person as something you could not decide".to_owned(),
    };
    let note = format!(
        "@{name}, an agent you started, asks you this. You decide it: answer with agent_send to {name}, setting answers to {} and why. {RAISE_RULE}, setting for to {name}. If your turn ends with neither, {if_silent}.",
        question.id
    );
    (text, note)
}

/// When a lead raises something rather than deciding it, and how.
const RAISE_RULE: &str = "Raise it to the person only if it is irreversible or leaves this machine, spends money, is a call of taste or product direction, or needs access only they have: use ask with reason, choices, pick and breaks";

/// The question as its lead reads it when it comes back, after a turn that left it unanswered.
fn for_lead_again(question: &Question) -> (String, String) {
    let (text, _) = for_lead(question);
    let name = &question.name;
    let note = format!(
        "You ended your turn without answering @{name}, which is still waiting. Decide it now: answer with agent_send to {name}, setting answers to {} and why. {RAISE_RULE}, setting for to {name}. If this turn ends with neither, it goes to the person as something you could not decide.",
        question.id
    );
    (text, note)
}

/// Hands a question that went to the asker's lead to it, or to the person
/// when the lead cannot take it.
async fn hand_to_lead(core: &Arc<Core>, question: &Question) {
    let (text, note) = for_lead(question);
    lead_reads(core, question, text, note).await;
}

async fn lead_reads(core: &Arc<Core>, question: &Question, text: String, note: String) {
    let Some(lead_id) = question.lead_id().map(str::to_owned) else {
        return;
    };
    if messages::question(
        core,
        question.id.clone(),
        question.agent_id.clone(),
        lead_id,
        text,
        note,
    )
    .await
    .is_ok()
    {
        return;
    }
    let mut state = core.harness.questions.state.locked();
    if let Some(open) = state.open.iter_mut().find(|open| open.id == question.id) {
        open.stage = Stage::Person {
            lead: None,
            passed: false,
        };
        core.harness.questions.changed(&mut state);
    }
}

const NOBODY_ASKED_ABOUT_CONTRACT: &str =
    "Nobody is asked about a contract. Decide from the card and give your verdict with review_verdict.";

pub(crate) async fn ask(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    let agent_id = request
        .agent_id
        .clone()
        .ok_or("ask needs the calling agent's id")?;
    if super::board::reviews_contract(core, &agent_id) {
        return Err(NOBODY_ASKED_ABOUT_CONTRACT.into());
    }
    let params = &request.params;
    let text = text_param(params, "question", MAX_QUESTION_BYTES)?
        .ok_or("question must be what you want to know, as nonempty text")?;
    let mut choices = choices(params.get("choices"))?;
    let header = text_param(params, "header", MAX_QUESTION_BYTES)?
        .map(|header| clipped(&header, MAX_HEADER_CHARS));
    if let Some(worker) = text_param(params, "for", 200)? {
        let raise = raise_of(params, &text, &mut choices)?;
        if let Some(foreign) = foreign_worker(core, &agent_id, &worker) {
            let choices = serde_json::to_value(&choices).map_err(|error| error.to_string())?;
            return super::hosts::route::pass_on(core, &agent_id, &foreign, text, choices, raise)
                .await;
        }
        return pass_on(core, &agent_id, &worker, text, choices, raise);
    }
    let blocked = match params.get("blocked") {
        None | Some(Value::Null) => true,
        Some(Value::Bool(blocked)) => *blocked,
        Some(_) => return Err("blocked must be a boolean".into()),
    };
    let assumption = text_param(params, "assumption", MAX_QUESTION_BYTES)?;
    match (blocked, &assumption) {
        (true, Some(_)) => {
            return Err(
                "assumption goes with blocked: false, when you carry on without the answer".into(),
            )
        }
        (false, None) => {
            return Err(
                "blocked: false needs assumption: what you go on assuming until you hear back"
                    .into(),
            )
        }
        _ => {}
    }
    let multi = match params.get("multi") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(multi)) => *multi,
        Some(_) => return Err("multi must be a boolean".into()),
    };
    if multi && choices.is_empty() {
        return Err("multi lets the person pick several choices, so it needs choices".into());
    }
    let secret = match params.get("secret") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(secret)) => *secret,
        Some(_) => return Err("secret must be a boolean".into()),
    };
    if secret && !blocked {
        return Err("a secret question is blocked: wait for the answer".into());
    }
    let raised = !secret
        && reachable_lead(core, &agent_id).is_none()
        && (params.get("reason").is_some() || (blocked && raises(core, &agent_id)));
    let raise = raised
        .then(|| raise_of(params, &text, &mut choices))
        .transpose()?;
    mark_pick(params, &mut choices)?;
    let question = open(
        core,
        &agent_id,
        &request.project,
        vec![Asked {
            text,
            header,
            choices,
            multi,
            blocked,
            assumption,
            secret,
            approval: None,
            dialog: None,
            raise,
        }],
    )?
    .remove(0);
    hand_to_lead(core, &question).await;
    let question = core
        .harness
        .questions
        .state
        .locked()
        .open
        .iter()
        .find(|open| open.id == question.id)
        .cloned()
        .unwrap_or(question);
    Ok(json!({
        "questionId": question.id,
        "askedOf": match &question.stage {
            Stage::Lead { lead_name, .. } => lead_name.clone(),
            Stage::Person { .. } if question.stands() => "nobody".to_owned(),
            Stage::Person { .. } => "the person".to_owned(),
        },
        "note": sent_note(&question),
    }))
}

/// Asks the person, never a lead, whether a plugin tool call goes ahead.
pub(crate) fn put_to_person(
    core: &Core,
    agent_id: &str,
    project: &str,
    header: String,
    text: String,
    choices: Vec<QuestionChoice>,
    approval: Approval,
) -> Result<(), String> {
    open(
        core,
        agent_id,
        project,
        vec![Asked {
            text: clipped(&text, MAX_QUESTION_BYTES),
            header: Some(clipped(&header, MAX_HEADER_CHARS)),
            choices,
            multi: false,
            blocked: true,
            assumption: None,
            secret: false,
            approval: Some(approval),
            dialog: None,
            raise: None,
        }],
    )
    .map(drop)
}

/// Asks the person something a dialog in `agent_id`'s process is blocked on.
pub(crate) fn ask_for_dialog(
    core: &Core,
    agent_id: &str,
    dialog: String,
    header: &str,
    text: &str,
    choices: Vec<QuestionChoice>,
) -> Result<(), String> {
    let project = core
        .chats
        .get(agent_id)
        .map(|chat| chat.launch.cwd.to_string_lossy().into_owned())
        .unwrap_or_default();
    open(
        core,
        agent_id,
        &project,
        vec![Asked {
            text: clipped(text, MAX_QUESTION_BYTES),
            header: Some(clipped(header, MAX_HEADER_CHARS)),
            choices,
            multi: false,
            blocked: true,
            assumption: None,
            secret: false,
            approval: None,
            dialog: Some(dialog),
            raise: None,
        }],
    )
    .map(drop)
}

/// Closes the question on `agent_id`'s dialog `only`, or on all its dialogs,
/// when they no longer wait.
pub(crate) fn withdraw_dialogs(core: &Core, agent_id: &str, only: Option<&str>) {
    let questions = &core.harness.questions;
    let mut state = questions.state.locked();
    let before = state.open.len();
    state.open.retain(|question| {
        let dialog = question.dialog.as_deref();
        question.agent_id != agent_id
            || dialog.is_none()
            || only.is_some_and(|only| dialog != Some(only))
    });
    if state.open.len() != before {
        questions.changed(&mut state);
    }
}

/// The worker of `lead` on a linked host that `worker` names, by its id or
/// name.
fn foreign_worker(core: &Core, lead: &str, worker: &str) -> Option<super::hosts::team::Foreign> {
    let wanted = worker.trim().trim_start_matches('@');
    let wanted = wanted.split([' ', '@']).next().unwrap_or(wanted);
    core.hosts.team.all().into_iter().find(|foreign| {
        foreign.parent.as_deref() == Some(lead)
            && (foreign.agent_id == wanted || foreign.name.eq_ignore_ascii_case(wanted))
    })
}

/// A lead on a linked host passes on to the person a question its worker
/// here asked it.
pub(crate) fn pass_on_from(
    core: &Core,
    lead: &str,
    worker: &str,
    text: &str,
    choices: &Value,
    raise: Raise,
) -> Result<Value, String> {
    let text = text_param(&json!({ "text": text }), "text", MAX_QUESTION_BYTES)?
        .ok_or("the question must be nonempty text")?;
    let choices = self::choices(Some(choices))?;
    pass_on(core, lead, worker, text, choices, raise)
}

/// A lead raises a question its agent asked it to the person.
fn pass_on(
    core: &Core,
    lead_id: &str,
    worker: &str,
    text: String,
    choices: Vec<QuestionChoice>,
    raise: Raise,
) -> Result<Value, String> {
    let wanted = worker.trim().trim_start_matches('@');
    let lead_name = core
        .names
        .name(lead_id)
        .unwrap_or_else(|| lead_id.to_owned());
    let mut state = core.harness.questions.state.locked();
    let from = |question: &Question| {
        question.agent_id == wanted || question.name.eq_ignore_ascii_case(wanted)
    };
    let Some(question) = state
        .open
        .iter_mut()
        .find(|question| from(question) && question.lead_id() == Some(lead_id))
    else {
        let asked_already = state.open.iter().any(|question| {
            from(question)
                && matches!(&question.stage, Stage::Person { lead: Some((lead, _)), .. } if lead == lead_id)
        });
        return if asked_already {
            Ok(
                json!({ "note": format!("The person already has {wanted}'s question; carry on with your own work.") }),
            )
        } else {
            Err(format!(
                "{wanted} has no question waiting on you; leave out for to ask the person something yourself"
            ))
        };
    };
    question.text = text;
    if !choices.is_empty() {
        question.choices = choices;
    }
    question.raise = Some(raise);
    question.stage = Stage::Person {
        lead: Some((lead_id.to_owned(), lead_name)),
        passed: true,
    };
    let (id, name) = (question.id.clone(), question.name.clone());
    core.harness.questions.changed(&mut state);
    Ok(json!({
        "questionId": id,
        "askedOf": "the person",
        "for": name,
        "note": format!("Sikemux asks the person for {name}. {name} gets the answer and you get a copy; carry on with your own work."),
    }))
}

/// Takes a question asked with a CLI's own tool, such as Claude's
/// AskUserQuestion or Codex's request_user_input, into the same questions,
/// and says what the agent is told in place of an answer.
pub(crate) fn ask_from_cli(core: &Arc<Core>, agent_id: &str, input: &Value) -> String {
    if super::board::reviews_contract(core, agent_id) {
        return NOBODY_ASKED_ABOUT_CONTRACT.to_owned();
    }
    let asked: Vec<Asked> = input
        .get("questions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|question| {
            let text = question.get("question")?.as_str()?.trim();
            if text.is_empty() || text.len() > MAX_QUESTION_BYTES {
                return None;
            }
            let options = question
                .get("options")
                .and_then(Value::as_array)
                .map(|options| options.iter().take(MAX_CHOICES).cloned().collect());
            let header = question
                .get("header")
                .and_then(Value::as_str)
                .filter(|header| !header.trim().is_empty())
                .map(|header| clipped(header, MAX_HEADER_CHARS));
            Some(Asked {
                text: text.to_owned(),
                header,
                multi: question["multiSelect"].as_bool().unwrap_or(false),
                choices: choices(options.map(Value::Array).as_ref()).unwrap_or_default(),
                blocked: true,
                assumption: None,
                secret: question["isSecret"].as_bool().unwrap_or(false),
                approval: None,
                dialog: None,
                raise: None,
            })
        })
        .collect();
    if asked.is_empty() {
        return "Sikemux could not read that question; ask it again with the ask tool.".into();
    }
    if raises(core, agent_id) && asked.iter().any(|asked| !asked.secret) {
        return format!(
            "This did not reach the person. You lead other agents, so it is yours to decide. {RAISE_RULE}."
        );
    }
    let project = core
        .chats
        .get(agent_id)
        .map(|chat| chat.launch.cwd.to_string_lossy().into_owned())
        .unwrap_or_default();
    let opened = match open(core, agent_id, &project, asked) {
        Ok(opened) => opened,
        Err(error) => return error,
    };
    let to = match opened.first().map(|question| &question.stage) {
        Some(Stage::Lead { lead_name, .. }) => format!("{lead_name}, your lead,"),
        _ => "the person".to_owned(),
    };
    let core = core.clone();
    tokio::spawn(async move {
        for question in &opened {
            hand_to_lead(&core, question).await;
        }
    });
    format!(
        "Sikemux put this question to {to} in place of this tool, so do not ask it again. Stop here and end your turn without guessing: the answer comes to you as your next message."
    )
}

/// The chat's turn ended by itself, so what its reply asks the person waits on them as a
/// question, in place of one it asked so before. An offer a yes or a no answers does not: the
/// agent is sent back to it when a project rule covers it, and otherwise it only shows as how
/// the turn left things. An agent with a lead, here or on a linked host, or that Auto runs,
/// answers to them instead, and one with a question, an answer or a message still on its way, or
/// at work again already, is not done. The questions' lock is held across the last look, so a
/// turn that starts meanwhile takes the question back.
pub(crate) fn asked_in_reply(core: &Arc<Core>, agent_id: &str) {
    let Some(chat) = core.chats.get(agent_id) else {
        return;
    };
    let named = core.names.get(agent_id);
    if named.as_ref().is_some_and(|named| named.parent.is_some())
        || super::board::run_by_auto(core, agent_id)
        || core.harness.mailbox.has_waiting(agent_id)
        || chat.feed.has_queued()
    {
        return;
    }
    let Some(reply) = chat.feed.last_answer() else {
        return;
    };
    let (offers, asks): (Vec<Ask>, Vec<Ask>) =
        asked_in(&reply).into_iter().partition(|ask| ask.polar);
    let name = (named.as_ref()).map_or_else(|| agent_id.to_owned(), |named| named.name.clone());
    let project = (named.map(|named| named.project))
        .unwrap_or_else(|| core.workspaces.project_of(&chat.launch.cwd));
    follow_rule(
        core,
        agent_id,
        &project,
        offers.into_iter().map(|offer| offer.text),
    );
    let was_sent_back = (core.harness.questions.state.locked().sent_back).remove(agent_id);
    if asks.is_empty() {
        return;
    }
    let started_by = (chat.feed.turn_prompt().as_deref()).and_then(messages::sent_by_agent);
    let leaning = asks.iter().any(|ask| leans(&ask.text));
    let choosing = asks.iter().any(|ask| ask.choice.is_some());
    if (started_by.is_some() || leaning || choosing) && !was_sent_back {
        let asked: Vec<&str> = asks.iter().map(|ask| ask.text.as_str()).collect();
        return send_back(core, agent_id, started_by.as_deref(), choosing, &asked);
    }
    let at = epoch_ms();
    let asked = (asks.into_iter().take(MAX_OPEN_PER_AGENT)).map(|ask| ReplyQuestion {
        id: format!("reply-{}", uuid::Uuid::new_v4().simple()),
        agent_id: agent_id.to_owned(),
        name: name.clone(),
        cli: chat.provider().to_owned(),
        project: project.clone(),
        text: clipped(&ask.text, MAX_QUESTION_BYTES),
        choices: ask.choice.map(reply_choices).unwrap_or_default(),
        given: None,
        at,
    });
    let questions = &core.harness.questions;
    let mut state = questions.state.locked();
    let asking = (state.open.iter())
        .chain(state.answered.iter().map(|answered| &answered.question))
        .any(|question| question.agent_id == agent_id);
    if asking || chat.working() {
        return;
    }
    state
        .reply_questions
        .retain(|asked| asked.agent_id != agent_id);
    state.reply_questions.extend(asked);
    questions.shown_changed(&mut state);
}

/// A choice the reply leaves to the person, as the options they pick from.
fn reply_choices(choice: Choice) -> Vec<QuestionChoice> {
    (choice.options.iter())
        .map(|option| QuestionChoice {
            label: clipped(option, MAX_LABEL_CHARS),
            detail: None,
            recommended: choice.pick.as_ref() == Some(option),
        })
        .collect()
}

/// Sends the agent back to the questions its turn ended with, which would not stand before the
/// person: the turn was another agent's to answer, having started with its message, a choice was
/// left to the person in prose rather than with `ask`, or a question says nothing without the
/// chat. Sent back once in a row, so asking again reaches the person.
fn send_back(
    core: &Arc<Core>,
    agent_id: &str,
    started_by: Option<&str>,
    choosing: bool,
    asked: &[&str],
) {
    (core.harness.questions.state.locked().sent_back).insert(agent_id.to_owned());
    let asked = asked.join("\n");
    let why = match started_by {
        Some(other) => {
            let other = core.names.name(other).unwrap_or_else(|| other.to_owned());
            format!("A message from @{other} started that turn, so what it raises is between you and @{other}: settle it with agent_send, or decide it yourself if it is yours. {RAISE_RULE}, saying what is being decided so it makes sense on its own.")
        }
        None if choosing => format!("A choice for the person goes through ask, never only in your reply, so it did not reach them. Decide it yourself if it is yours. {RAISE_RULE}, with a question that says what is being decided so it makes sense without your chat, a header of two or three words and the choice you recommend first, then end your turn without writing the choice out in your reply again."),
        None => format!("It says nothing without your chat, so it did not reach the person. Decide it yourself if it is yours. {RAISE_RULE}, saying what is being decided so it makes sense on its own."),
    };
    let text = format!("Your turn ended asking:\n\n{asked}\n\n{why}");
    let (core, agent_id) = (core.clone(), agent_id.to_owned());
    tokio::spawn(async move {
        if let Err(error) = messages::ask_again(&core, agent_id, text).await {
            eprintln!("sikemux core: an agent was not sent back to its questions: {error}");
        }
    });
}

/// The first offer of more work in an agent's reply, which a yes or a no answers.
pub(crate) fn offer_in(reply: &str) -> Option<String> {
    let mut asks = asked_in(reply).into_iter();
    asks.find_map(|ask| ask.polar.then_some(ask.text))
}

/// Sends the agent back to the first of its offers a project rule is about, for the rule to say
/// whether it does the work or leaves it for the person. A rule sends it back once in a row, so
/// offering it again leaves it as state.
fn follow_rule(
    core: &Arc<Core>,
    agent_id: &str,
    project: &str,
    offers: impl Iterator<Item = String>,
) {
    let covered = offers.into_iter().find_map(|offer| {
        let rule = super::memory::rule_about(core, project, &offered_work(&offer)?)?;
        Some((offer, rule))
    });
    let Some((offer, rule)) = covered else {
        let followed = &mut core.harness.questions.state.locked().followed_rules;
        followed.remove(agent_id);
        return;
    };
    let mut state = core.harness.questions.state.locked();
    if state.followed_rules.get(agent_id) == Some(&rule) {
        return;
    }
    state
        .followed_rules
        .insert(agent_id.to_owned(), rule.clone());
    drop(state);
    let (core, agent_id) = (core.clone(), agent_id.to_owned());
    tokio::spawn(async move {
        let text = format!(
            "You ended your turn offering: {offer}\n\nA rule of this project is about that: {rule}\n\nThe rule decides, not this note. If it tells you to do the work, do it. If it says to wait for the person's word, or not to do it, leave the work as it is and end your turn without offering it again: the person sees how you left it on your row and takes it up in your chat. What leaves this machine still stops for them."
        );
        if let Err(error) = messages::over_rule(&core, agent_id, text).await {
            eprintln!("sikemux core: an agent was not sent back over a rule: {error}");
        }
    });
}

/// The open questions and the answers their askers have not read, for the
/// app, the notch and phones.
pub(crate) fn views(core: &Core) -> Vec<AgentQuestion> {
    let (open, answered, reply_questions) = {
        let state = core.harness.questions.state.locked();
        (
            state.open.clone(),
            state.answered.clone(),
            state.reply_questions.clone(),
        )
    };
    views_of(core, open, answered, reply_questions)
}

/// Each question's view, then each asked in a reply, with the ticket of the card its asker holds.
fn views_of(
    core: &Core,
    open: Vec<Question>,
    answered: Vec<Answered>,
    reply_questions: Vec<ReplyQuestion>,
) -> Vec<AgentQuestion> {
    let open = open.iter().map(Question::view);
    let answered = answered.iter().map(|answered| {
        let mut view = answered.question.view();
        view.state = QuestionState::Answered;
        view.answer = answered.answer.clone();
        view
    });
    open.chain(answered)
        .chain(
            (reply_questions.iter())
                .filter(|question| question.waits())
                .map(ReplyQuestion::view),
        )
        .map(|mut view| {
            view.ticket = super::board::ticket_of_agent(core, &view.agent_id);
            view
        })
        .collect()
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnswerParams {
    pub id: String,
    #[serde(default)]
    pub answer: Option<String>,
    /// Files attached to `answer`, by where the host keeps them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    /// What was sent with `answer`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context: Vec<ChatContext>,
    #[serde(default)]
    pub choice: Option<usize>,
    /// The choices picked on a question that takes several.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub choices: Option<Vec<usize>>,
    /// The assumption the agent went on is right.
    #[serde(default)]
    pub fine: bool,
}

/// The person's answer: the first one wins, and reaches the asker as a
/// message.
pub(crate) async fn answer(core: &Arc<Core>, params: AnswerParams) -> Result<(), String> {
    answer_all(
        core,
        AnswersParams {
            answers: vec![params],
        },
    )
    .await
}

#[derive(Serialize, Deserialize)]
pub struct AnswersParams {
    pub answers: Vec<AnswerParams>,
}

impl AnswerParams {
    /// Writes the files and context into `answer`: what was typed, the files, then the context,
    /// so the agent reads the same text whichever client answered.
    fn written_out(&mut self) {
        let paths = std::mem::take(&mut self.paths);
        let context = std::mem::take(&mut self.context);
        if paths.is_empty() && context.is_empty() {
            return;
        }
        let attached = (!paths.is_empty()).then(|| {
            let listed: Vec<String> = paths.iter().map(|path| format!("- {path}")).collect();
            format!("Attached:\n{}", listed.join("\n"))
        });
        let context = (context.iter()).map(|item| match item.text.is_empty() {
            true => item.title.clone(),
            false => format!("{}:\n{}", item.title, item.text),
        });
        let parts = (self.answer.take().into_iter())
            .chain(attached)
            .chain(context)
            .filter(|part| !part.is_empty());
        self.answer = Some(parts.collect::<Vec<_>>().join("\n\n"));
    }
}

/// The person's answers to questions asked together. Each asker gets all of
/// its answers in one message, so the first does not start a turn before the
/// rest arrive.
pub(crate) async fn answer_all(core: &Arc<Core>, mut params: AnswersParams) -> Result<(), String> {
    params
        .answers
        .iter_mut()
        .for_each(AnswerParams::written_out);
    let (reply_questions, answers): (Vec<AnswerParams>, Vec<AnswerParams>) = {
        let state = core.harness.questions.state.locked();
        (params.answers.into_iter()).partition(|given| {
            state
                .reply_questions
                .iter()
                .any(|question| question.id == given.id)
        })
    };
    answer_replies(core, reply_questions).await?;
    if answers.is_empty() {
        return Ok(());
    }
    let params = AnswersParams { answers };
    let taken = {
        let mut state = core.harness.questions.state.locked();
        let mut taken = Vec::with_capacity(params.answers.len());
        for given in params.answers {
            let question = state
                .open
                .iter()
                .find(|question| question.id == given.id)
                .ok_or("That question has been answered already")?;
            let text = answer_text(question, &given)?;
            taken.push((question.clone(), given, text));
        }
        state
            .open
            .retain(|question| !taken.iter().any(|(taken, ..)| taken.id == question.id));
        for (question, _, text) in &taken {
            if let (None, None, Some(text)) = (&question.from_host, &question.dialog, text) {
                state.answered.push(Answered {
                    question: question.clone(),
                    answer: (!question.secret).then(|| text.clone()),
                });
            }
        }
        let over = state.answered.len().saturating_sub(MAX_ANSWERED);
        state.answered.drain(..over);
        core.harness.questions.changed(&mut state);
        taken
    };
    let reopen = |questions: Vec<Question>| {
        let mut state = core.harness.questions.state.locked();
        state.answered.retain(|answered| {
            !questions
                .iter()
                .any(|question| question.id == answered.question.id)
        });
        state.open.extend(questions);
        core.harness.questions.changed(&mut state);
    };
    // Each answer as the asker gets it, with what was done about it when that is not the answer itself.
    let mut letters: Vec<(Question, String, Option<String>)> = Vec::new();
    for (question, given, text) in taken {
        if let Some(host) = &question.from_host {
            let params = serde_json::to_value(&given).map_err(|error| error.to_string())?;
            super::hosts::route::relay_answer(core, host, params);
            continue;
        }
        let Some(text) = text else { continue };
        if let Some(dialog) = &question.dialog {
            if let Some(chat) = core.chats.get(&question.agent_id) {
                let _ = chat.send(ChatCommand::AnswerDialog {
                    id: dialog.clone(),
                    answer: text,
                    choice: given.choice,
                });
            }
            continue;
        }
        let told = match (&question.approval, question.secret) {
            (Some(approval), _) => super::approvals::settle(core, approval, given.choice)
                .await
                .map(|done| (text, Some(done))),
            (None, true) => core
                .harness
                .questions
                .keep_secret(&question, &text)
                .map(|path| (secret_note(&path), None)),
            (None, false) => Ok((text, None)),
        };
        match told {
            Ok((text, done)) => letters.push((question, text, done)),
            Err(error) => {
                reopen(
                    letters
                        .into_iter()
                        .map(|(question, ..)| question)
                        .chain([question])
                        .collect(),
                );
                return Err(error);
            }
        }
    }
    let mut askers: Vec<&str> = letters
        .iter()
        .map(|(question, ..)| question.agent_id.as_str())
        .collect();
    askers.sort_unstable();
    askers.dedup();
    let mut failed = None;
    for asker in askers {
        let own: Vec<&(Question, String, Option<String>)> = letters
            .iter()
            .filter(|(question, ..)| question.agent_id == asker)
            .collect();
        let sent = own
            .iter()
            .map(|(question, text, done)| {
                let note = match (done, &question.assumption) {
                    (Some(done), _) => done.clone(),
                    (None, Some(assumption)) => format!(
                        "The person answered the question you went on without. If that is not what you assumed ({assumption}), change course."
                    ),
                    (None, None) => "The person answered the question you asked; go on with it.".to_owned(),
                };
                (answered(question, &question.asked), text.clone(), note)
            })
            .collect();
        match messages::answer(core, asker.to_owned(), sent).await {
            Ok(delivered) if delivered["delivery"] == "waiting" => {}
            Ok(_) => core.harness.questions.delivered(asker),
            Err(error) => {
                reopen(
                    own.into_iter()
                        .map(|(question, ..)| question.clone())
                        .collect(),
                );
                failed = Some(error);
                continue;
            }
        }
        for (question, text, _) in own {
            copy_to_lead(core, question, text).await;
        }
    }
    match failed {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

/// The person's answers to what agents asked in their replies, typed or one of the one-tap
/// answers. Each is held until every question of its reply has one; then the agent hears them
/// all in one message, each answer with the question it answers.
async fn answer_replies(core: &Arc<Core>, given: Vec<AnswerParams>) -> Result<(), String> {
    if given.is_empty() {
        return Ok(());
    }
    let questions = &core.harness.questions;
    let ready: Vec<(String, Vec<ReplyQuestion>)> = {
        let mut state = questions.state.locked();
        let mut answers: Vec<(String, String)> = Vec::with_capacity(given.len());
        for given in &given {
            let question = (state.reply_questions.iter())
                .find(|question| question.id == given.id && question.waits())
                .ok_or("That has been answered already")?;
            let text = match given.choice {
                Some(choice) => (question.choices.get(choice))
                    .map(|choice| choice.label.clone())
                    .ok_or("That choice is not one of the question's")?,
                None => reply_text(given)?,
            };
            answers.push((question.id.clone(), text));
        }
        let mut askers: Vec<String> = Vec::new();
        for (id, text) in answers {
            if let Some(question) = (state.reply_questions.iter_mut()).find(|kept| kept.id == id) {
                question.given = Some(text);
                if !askers.contains(&question.agent_id) {
                    askers.push(question.agent_id.clone());
                }
            }
        }
        askers.retain(|asker| {
            (state.reply_questions.iter())
                .filter(|question| question.agent_id == *asker)
                .all(|question| !question.waits())
        });
        let ready = askers
            .into_iter()
            .map(|asker| {
                let (theirs, rest) = std::mem::take(&mut state.reply_questions)
                    .into_iter()
                    .partition(|question| question.agent_id == asker);
                state.reply_questions = rest;
                (asker, theirs)
            })
            .collect();
        questions.shown_changed(&mut state);
        ready
    };
    let note = "The person answered what you asked as your last turn ended; go on with it.";
    for (asker, answered) in ready {
        let sent = (answered.iter())
            .map(|question| {
                let text = question.given.clone().unwrap_or_default();
                (question.answered(), text, note.to_owned())
            })
            .collect();
        if let Err(error) = messages::answer(core, asker.clone(), sent).await {
            let mut state = questions.state.locked();
            if state
                .reply_questions
                .iter()
                .all(|kept| kept.agent_id != asker)
            {
                let reopened = answered.into_iter().map(|question| ReplyQuestion {
                    given: None,
                    ..question
                });
                state.reply_questions.extend(reopened);
                questions.shown_changed(&mut state);
            }
            return Err(error);
        }
    }
    Ok(())
}

/// The answers the person gave to what the agent asked in its last reply, each beside its
/// question, when the person writes to it before answering every question: they go with what
/// the person wrote, and the questions left unanswered no longer wait.
pub(crate) fn held_answers(core: &Core, agent_id: &str) -> Vec<ChatContext> {
    let questions = &core.harness.questions;
    let theirs: Vec<ReplyQuestion> = {
        let mut state = questions.state.locked();
        let (theirs, rest) = std::mem::take(&mut state.reply_questions)
            .into_iter()
            .partition(|question| question.agent_id == agent_id);
        state.reply_questions = rest;
        if !theirs.is_empty() {
            questions.shown_changed(&mut state);
        }
        theirs
    };
    (theirs.into_iter())
        .filter_map(|question| {
            let answered = format!("{}: {}", question.given?, question.text);
            Some(ChatContext {
                uri: format!("{}{}", sikemux_view::chat::ANSWER_URI, question.id),
                text: format!(
                    "{answered}\n\nThe person gave this answer to what you asked as your last turn ended, then wrote to you; go on with both."
                ),
                title: answered,
            })
        })
        .collect()
}

/// The person's answer, copied to the lead a question went to first.
async fn copy_to_lead(core: &Arc<Core>, question: &Question, text: &str) {
    if let (
        false,
        Stage::Person {
            lead: Some((lead_id, _)),
            ..
        },
    ) = (question.secret, &question.stage)
    {
        let note = format!(
            "The person answered {}'s question, which went to you first. {} has the answer too.",
            question.name, question.name
        );
        let _ = messages::answer(
            core,
            lead_id.clone(),
            vec![(answered(question, &question.text), text.to_owned(), note)],
        )
        .await;
    }
}

fn answered(question: &Question, question_text: &str) -> messages::Answered {
    messages::Answered {
        question: question_text.to_owned(),
        asker: question.name.clone(),
        asker_id: question.agent_id.clone(),
        cli: question.cli.clone(),
        secret: question.secret,
    }
}

/// What the agent is told in place of the secret the person answered with.
fn secret_note(path: &Path) -> String {
    let path = path.display();
    format!(
        "The person's answer is a secret, so Sikemux kept it out of this conversation. It is in {path}, which only the person's account can read. Use it straight from the file, such as \"$(cat '{path}')\" in a command, and never print it, echo it, or write it into a reply, a message or another file."
    )
}

/// What the person's answer says, or none when they only agreed with the
/// assumption the agent went on.
fn answer_text(question: &Question, params: &AnswerParams) -> Result<Option<String>, String> {
    if params.fine {
        return match question.assumption {
            Some(_) => Ok(None),
            None => Err("Only an assumption can be agreed with; answer the question".into()),
        };
    }
    if let Some(picked) = &params.choices {
        if !question.multi {
            return Err("That question takes one choice".into());
        }
        if picked.is_empty() {
            return Err("Pick at least one choice".into());
        }
        let labels = picked
            .iter()
            .map(|&choice| {
                question
                    .choices
                    .get(choice)
                    .map(|choice| choice.label.as_str())
                    .ok_or("That choice is not one of the question's")
            })
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(Some(labels.join(", ")));
    }
    if let Some(choice) = params.choice {
        return question
            .choices
            .get(choice)
            .map(|choice| Some(choice.label.clone()))
            .ok_or_else(|| "That choice is not one of the question's".into());
    }
    reply_text(params).map(Some)
}

/// The words the person wrote as their answer.
fn reply_text(params: &AnswerParams) -> Result<String, String> {
    match params.answer.as_deref().map(str::trim) {
        Some(text) if !text.is_empty() && text.len() <= MAX_ANSWER_BYTES => Ok(text.to_owned()),
        Some(text) if !text.is_empty() => Err(format!(
            "An answer holds at most {} KB",
            MAX_ANSWER_BYTES / 1024
        )),
        _ => Err("The answer is empty".into()),
    }
}

/// The first question of each agent's reply, which a phone is told of once for all of them.
fn first_of_each_reply(asked: &[ReplyQuestion]) -> impl Iterator<Item = &ReplyQuestion> {
    asked.iter().enumerate().filter_map(|(at, question)| {
        let first = !asked[..at]
            .iter()
            .any(|earlier| earlier.agent_id == question.agent_id);
        first.then_some(question)
    })
}

/// Moves on questions their lead left, drops those of agents that are gone,
/// and tells the window and phones what changed. Runs when questions or the
/// agents change, and again when a lead's time to answer runs out.
pub(crate) fn settle(core: &Arc<Core>) {
    let (askers, leads): (HashSet<String>, HashSet<String>) = {
        let state = core.harness.questions.state.locked();
        if state.dirty {
            core.boards.wake.now();
        }
        if state.open.is_empty()
            && state.answered.is_empty()
            && state.reply_questions.is_empty()
            && state.told_phones.is_empty()
            && state.shown_devices.is_empty()
            && !state.dirty
        {
            drop(state);
            tell_window(core);
            return;
        }
        (
            state
                .open
                .iter()
                .chain(state.answered.iter().map(|answered| &answered.question))
                .map(|question| question.agent_id.clone())
                .chain(
                    state
                        .reply_questions
                        .iter()
                        .map(|question| question.agent_id.clone()),
                )
                .collect(),
            state
                .open
                .iter()
                .filter_map(|question| question.lead_id().map(str::to_owned))
                .collect(),
        )
    };
    let gone: HashSet<String> = askers
        .into_iter()
        .filter(|agent_id| gone(core, agent_id))
        .collect();
    let leads: HashMap<String, (bool, bool)> = leads
        .into_iter()
        .map(|lead| {
            let state = (
                reachable(core, &lead),
                core.agent_now(&lead).maybe_mid_turn(),
            );
            (lead, state)
        })
        .collect();
    let unread: HashSet<String> = {
        let state = core.harness.questions.state.locked();
        state
            .open
            .iter()
            .filter_map(|question| {
                let lead = question.lead_id()?;
                messages::is_waiting(core, lead, &question.id).then(|| question.id.clone())
            })
            .collect()
    };
    let now = epoch_ms();
    let mut withdrawn = Vec::new();
    let mut next = None;
    let mut closed = Vec::new();
    let mut again = Vec::new();
    let standing: Vec<Question>;
    {
        let mut state = core.harness.questions.state.locked();
        let before = state.open.len();
        state.open.retain(|question| {
            let keep = !gone.contains(&question.agent_id);
            if !keep {
                if let Some(host) = &question.relayed {
                    closed.push((host.clone(), question.id.clone()));
                }
            }
            keep
        });
        let answered = state.answered.len();
        state
            .answered
            .retain(|answered| !gone.contains(&answered.question.agent_id));
        let reply_questions = state.reply_questions.len();
        state
            .reply_questions
            .retain(|question| !gone.contains(&question.agent_id));
        if state.reply_questions.len() != reply_questions {
            core.harness.questions.shown_changed(&mut state);
        }
        let mut changed = state.open.len() != before || state.answered.len() != answered;
        for question in &mut state.open {
            let Stage::Lead {
                lead_id,
                lead_name,
                ended_at,
                handed_back,
            } = &mut question.stage
            else {
                continue;
            };
            let (reachable, busy) = leads
                .get(lead_id.as_str())
                .copied()
                .unwrap_or((false, false));
            let unread = unread.contains(&question.id);
            let passed_over =
                ended_at.is_some_and(|at| now.saturating_sub(at) >= LEAD_GRACE_MS) && !unread;
            if busy {
                *ended_at = None;
            } else if reachable && passed_over && !*handed_back && question.assumption.is_none() {
                *ended_at = None;
                *handed_back = true;
                again.push(question.clone());
                changed = true;
            } else if !reachable || passed_over {
                if unread {
                    withdrawn.push((lead_id.clone(), question.id.clone()));
                }
                question.stage = Stage::Person {
                    lead: (!unread).then(|| (lead_id.clone(), lead_name.clone())),
                    passed: false,
                };
                changed = true;
            } else if let Some(at) = ended_at.filter(|_| !unread) {
                next = sooner(next, Some(at + LEAD_GRACE_MS));
            }
        }
        let (stands, open) = std::mem::take(&mut state.open)
            .into_iter()
            .partition::<Vec<_>, _>(Question::stands);
        state.open = open;
        changed |= !stands.is_empty();
        standing = stands;
        if changed {
            core.harness.questions.changed(&mut state);
            core.boards.wake.now();
        }
    }
    for question in standing {
        stood(core, question);
    }
    for question in again {
        let core = core.clone();
        tokio::spawn(async move {
            let (text, note) = for_lead_again(&question);
            lead_reads(&core, &question, text, note).await;
        });
    }
    for (lead, question) in withdrawn {
        messages::withdraw(core, &lead, &question);
    }
    for (host, id) in closed {
        super::hosts::route::relay_closed(core, &host, &id);
    }
    relay(core);
    if let Some(next) = next {
        core.harness.questions.wake.at(next);
    }
    tell_phones(core);
    tell_window(core);
}

/// A question that waits on the person, as phones are told of it.
#[derive(Clone)]
struct Asking {
    agent_id: String,
    name: String,
    text: String,
    at: u64,
}

impl Asking {
    fn new(agent_id: &str, name: &str, text: &str, at: u64) -> Self {
        Self {
            agent_id: agent_id.to_owned(),
            name: name.to_owned(),
            text: text.to_owned(),
            at,
        }
    }
}

/// Tells phones of each question that now waits on the person, and of each
/// that no longer does. An agent whose questions to the person changed counts
/// as active, which changes the view devices are sent, so they read the
/// questions again.
fn tell_phones(core: &Core) {
    let (asked, cleared, changed) = {
        let mut state = core.harness.questions.state.locked();
        let shown: HashMap<String, String> = state
            .open
            .iter()
            .filter(|question| question.asks_person())
            .map(|question| (question.id.clone(), question.agent_id.clone()))
            .chain(
                (state.reply_questions.iter())
                    .filter(|question| question.waits())
                    .map(|question| (question.id.clone(), question.agent_id.clone())),
            )
            .collect();
        let changed: HashSet<String> = shown
            .iter()
            .filter(|(id, _)| !state.shown_devices.contains_key(*id))
            .chain(
                state
                    .shown_devices
                    .iter()
                    .filter(|(id, _)| !shown.contains_key(*id)),
            )
            .map(|(_, agent_id)| agent_id.clone())
            .collect();
        state.shown_devices = shown;
        let now: HashMap<String, Asking> = (state.open.iter())
            .filter(|question| {
                let waits = question.blocked || question.raise.is_some() || question.persons_call();
                waits && question.asks_person()
            })
            .map(|question| {
                let asking = Asking::new(
                    &question.agent_id,
                    &question.name,
                    &question.text,
                    question.at,
                );
                (question.id.clone(), asking)
            })
            .chain(first_of_each_reply(&state.reply_questions).map(|question| {
                let asking = Asking::new(
                    &question.agent_id,
                    &question.name,
                    &question.text,
                    question.at,
                );
                (question.id.clone(), asking)
            }))
            .collect();
        let asked: Vec<(String, Asking)> = (now.iter())
            .filter(|(id, _)| !state.told_phones.contains_key(*id))
            .map(|(id, asking)| (id.clone(), asking.clone()))
            .collect();
        let cleared: Vec<(String, String)> = state
            .told_phones
            .iter()
            .filter(|(id, _)| !now.contains_key(*id))
            .map(|(id, agent_id)| (id.clone(), agent_id.clone()))
            .collect();
        state.told_phones = now
            .into_iter()
            .map(|(id, asking)| (id, asking.agent_id))
            .collect();
        (asked, cleared, changed)
    };
    for agent_id in changed {
        core.seen.active(&agent_id);
    }
    for (id, asking) in asked {
        core.note_activity(&asking.agent_id, Activity::NeedsYou);
        core.notify.send(Signal::Question {
            id,
            agent_id: asking.agent_id,
            name: asking.name,
            text: asking.text,
            at: asking.at,
        });
    }
    for (id, agent_id) in cleared {
        core.notify.send(Signal::QuestionCleared { id, agent_id });
    }
}

/// Sends the window the open questions when they changed, or when a window
/// opened that has not had them.
fn tell_window(core: &Arc<Core>) {
    let window = core.window.client_id();
    let (open, answered, reply_questions) = {
        let mut state = core.harness.questions.state.locked();
        if !state.dirty && state.told_window == window {
            return;
        }
        if state.dirty {
            core.changed(What::View);
            core.changed(What::Read(Topic::Questions));
        }
        state.dirty = false;
        state.told_window = window;
        if window.is_none() {
            return;
        }
        (
            state.open.clone(),
            state.answered.clone(),
            state.reply_questions.clone(),
        )
    };
    let views = views_of(core, open, answered, reply_questions);
    let request = json!({
        "id": "agent-questions",
        "project": "/",
        "agentId": null,
        "method": "agent.questions",
        "params": { "questions": views },
    });
    let Ok(call) = core.window.call(
        WindowCall::Harness { request },
        "show agents' questions",
        false,
        None,
    ) else {
        return;
    };
    let id = call.id;
    let core = core.clone();
    tokio::spawn(async move {
        if tokio::time::timeout(WINDOW_ANSWER_WAIT, call.answer)
            .await
            .is_err()
        {
            core.window.forget(id);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::reply::{Ask, Choice};

    fn asked_in(reply: &str) -> Vec<(String, bool)> {
        (super::asked_in(reply).into_iter())
            .map(|ask| (ask.text, ask.polar))
            .collect()
    }

    /// The shape of a reply whose three offers showed as one question, cut inside its bold.
    const THREE_OFFERS: &str = "The claims read is the slow part now.\n\n\
        - Text-ad Projects load in 1.8 s. **Ship it to dev now,** alongside the worker deploy \
        that's still waiting on your approval?\n\
        - Ship the speed fix to dev?\n\
        - **Profile the claims read** to get text-ad Projects under a second?\n";

    #[test]
    fn each_ask_of_a_reply_is_a_question_of_its_own_in_plain_words() {
        let asked = asked_in(THREE_OFFERS);
        assert_eq!(
            asked,
            vec![
                (
                    "Ship it to dev now, alongside the worker deploy that's still waiting on \
                     your approval?"
                        .to_owned(),
                    true
                ),
                ("Ship the speed fix to dev?".to_owned(), true),
                (
                    "Profile the claims read to get text-ad Projects under a second?".to_owned(),
                    true
                ),
            ]
        );
    }

    #[test]
    fn no_sentence_ends_inside_emphasis_or_code() {
        let asked = asked_in("Tests pass. **Merged, then deployed.** Which of `a. b` or `c` next?");
        assert_eq!(asked, vec![("Which of a. b or c next?".to_owned(), false)]);
        let asked = asked_in("Tests pass. **Merged, then deployed. Check it?**");
        assert_eq!(
            asked,
            vec![("Merged, then deployed. Check it?".to_owned(), true)]
        );
    }

    /// The shape of a reply whose question showed as "Yes or no?", with no Yes or No to tap.
    const BARE_LAST_SENTENCE: &str = "**Your call: fix the repeated number now?** It prints \
        twice on any shape with a big number.\n\n\
        The catch is the cost of recomposing. This is the cheapest time to do it. Yes or no?\n";

    #[test]
    fn a_last_sentence_that_means_nothing_alone_shows_the_question_it_stands_for() {
        assert_eq!(
            asked_in(BARE_LAST_SENTENCE),
            vec![("Your call: fix the repeated number now?".to_owned(), true)]
        );
        assert_eq!(
            asked_in("Should I fix it?\n\nYes or no?"),
            vec![("Should I fix it?".to_owned(), true)]
        );
        assert_eq!(
            asked_in("It's fixed. Yes or no?"),
            vec![("It's fixed. Yes or no?".to_owned(), true)]
        );
    }

    /// The shape of a reply whose question showed as "Is that okay?", saying nothing of what.
    const OKAY_AFTER_A_PROPOSAL: &str = "**Who owns #5 (no indication on Product claim stills)?** \
        I'd have Satoru find the cause first, and Medical or you decide whether it blocks. \
        Is that okay?\n";

    #[test]
    fn a_question_about_what_was_just_said_shows_with_it() {
        assert_eq!(
            asked_in(OKAY_AFTER_A_PROPOSAL),
            vec![(
                "Who owns #5 (no indication on Product claim stills)? I'd have Satoru find the \
                 cause first, and Medical or you decide whether it blocks. Is that okay?"
                    .to_owned(),
                false
            )]
        );
        assert_eq!(
            asked_in("We could cache it.\n\nThoughts?"),
            vec![("We could cache it. Thoughts?".to_owned(), false)]
        );
    }

    fn choice(text: &str, options: &[&str], pick: Option<&str>) -> Ask {
        Ask {
            text: text.to_owned(),
            polar: false,
            choice: Some(Choice {
                options: options.iter().map(|option| (*option).to_owned()).collect(),
                pick: pick.map(str::to_owned),
            }),
        }
    }

    /// The endings of three replies whose choice between two mockups never reached Needs you.
    const OTHER_OPEN_CALL: &str = "Want me to make it automatic and remove the prompt, or drop \
        chat-to-goal tracking entirely? Your A/B pick on the Home mockup is still the other open \
        call.";
    const LEANING: &str = "Two Home mockups are on your desk.\n\n\
        - **A** — one board for every goal\n\
        - **B** — goals split by project\n\n\
        Which one? I'm leaning A. I won't touch the layout until you pick.";
    const SAY_A_OR_B: &str =
        "Say \"A\" (my pick) or \"B\" and I'll start teammates on the redesign…";
    const PICK_ONE: &str = "Two Home mockups are on your desk.\n\n\
        ### A: one board\n\nEvery goal on one board.\n\n\
        ### B: by project\n\nGoals split by project.\n\n\
        Pick one, or point at what you'd change.";

    #[test]
    fn a_choice_left_in_prose_is_asked_with_its_options_and_the_agents_pick() {
        assert_eq!(
            super::asked_in(OTHER_OPEN_CALL),
            vec![choice(
                "Want me to make it automatic and remove the prompt, or drop chat-to-goal \
                 tracking entirely? Your A/B pick on the Home mockup is still the other open call.",
                &[
                    "Make it automatic and remove the prompt",
                    "Drop chat-to-goal tracking entirely"
                ],
                None,
            )]
        );
        assert_eq!(
            super::asked_in(LEANING),
            vec![choice(
                "Which one? I'm leaning A. I won't touch the layout until you pick.",
                &["A", "B"],
                Some("A"),
            )]
        );
        assert_eq!(
            super::asked_in(SAY_A_OR_B),
            vec![choice(
                "Say \"A\" (my pick) or \"B\" and I'll start teammates on the redesign…",
                &["A", "B"],
                Some("A"),
            )]
        );
        assert_eq!(
            super::asked_in(PICK_ONE),
            vec![choice(
                "Pick one, or point at what you'd change.",
                &["A", "B"],
                None
            )]
        );
        assert_eq!(
            asked_in("Run make dev to get your build back. Then say if it looks right."),
            Vec::new()
        );
    }
}
