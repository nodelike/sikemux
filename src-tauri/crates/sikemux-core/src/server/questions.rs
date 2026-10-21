//! What `ask` answers, and the CLIs' own question tools with it. An agent's
//! question goes first to the agent that started it, and to the person when
//! that agent passes it on, has stopped, or ends its turn without answering.
//! The person answers in the asker's chat, the notch or Sikemux's rail, and
//! the first answer reaches the asker as a message.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use crate::cli::protocol::HarnessRequest;
use crate::protocol::WindowCall;
use crate::text::clipped;

use super::activity::Activity;
use super::connection::ClientId;
use super::kept::Kept;
use super::messages;
use super::names::project_of;
use super::notify::Signal;
use super::wake::{sooner, Wake};
use super::Core;

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
/// A lead whose turn ended may still be handed the question back unread, so
/// it counts as passing it over only once this long has gone by.
const LEAD_GRACE_MS: u64 = 3_000;
const WINDOW_ANSWER_WAIT: Duration = Duration::from_secs(10);
/// What the agent reads as the answer to a question the person skipped.
const SKIPPED: &str = "The person skipped this question without answering it.";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Choice {
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "stage")]
enum Stage {
    /// Sent to the agent that started the asker. `ended_at` is when that
    /// agent's turn ended after it was handed the question.
    #[serde(rename_all = "camelCase")]
    Lead {
        lead_id: String,
        lead_name: String,
        ended_at: Option<u64>,
    },
    /// Put to the person. `lead` is the agent it went to first, which
    /// `passed` it on itself rather than leaving it unanswered.
    #[serde(rename_all = "camelCase")]
    Person {
        lead: Option<(String, String)>,
        passed: bool,
    },
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
    choices: Vec<Choice>,
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
}

impl Question {
    fn view(&self) -> Value {
        let (lead, via, passed) = match &self.stage {
            Stage::Lead { lead_name, .. } => (Some(lead_name.clone()), None, false),
            Stage::Person { lead, passed } => {
                (None, lead.as_ref().map(|(_, name)| name.clone()), *passed)
            }
        };
        json!({
            "id": self.id,
            "agentId": self.agent_id,
            "name": self.name,
            "cli": self.cli,
            "project": self.project,
            "asked": self.asked,
            "text": self.text,
            "header": self.header,
            "choices": self.choices,
            "multi": self.multi,
            "blocked": self.blocked,
            "assumption": self.assumption,
            "secret": self.secret,
            "at": self.at,
            "lead": lead,
            "via": via,
            "passed": passed,
        })
    }

    fn asks_person(&self) -> bool {
        matches!(self.stage, Stage::Person { .. }) && self.relayed.is_none()
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

#[derive(Default)]
struct State {
    open: Vec<Question>,
    /// Something changed that the window has not been sent yet.
    dirty: bool,
    /// The window connection last sent the questions.
    told_window: Option<ClientId>,
    /// The person's open questions phones were told of, and whose they are.
    told_phones: HashMap<String, String>,
    /// Every question put to the person, assumptions too, that devices'
    /// views last showed, and whose they are.
    shown_devices: HashMap<String, String>,
}

pub(crate) struct Questions {
    state: Mutex<State>,
    kept: Kept,
    secrets: Option<std::path::PathBuf>,
    /// Moves questions on and tells the window and phones of them.
    pub(crate) wake: Wake,
}

/// A question that waits on the person.
pub(crate) struct ForPerson {
    pub id: String,
    pub agent_id: String,
    pub text: String,
    pub blocked: bool,
    pub assumption: Option<String>,
    pub at: u64,
}

/// A question as an agent asked it.
struct Asked {
    text: String,
    header: Option<String>,
    choices: Vec<Choice>,
    multi: bool,
    blocked: bool,
    assumption: Option<String>,
    secret: bool,
}

impl Questions {
    /// Takes up the questions left open by agents `known` says still exist.
    pub(crate) fn new(data_dir: Option<&Path>, known: impl Fn(&str) -> bool) -> Self {
        let kept = Kept::new(data_dir, FILE);
        let mut record: QuestionsRecord = kept.load();
        record.open.retain(|question| known(&question.agent_id));
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
        state.dirty = true;
        self.kept.save(&QuestionsRecord {
            open: state.open.clone(),
        });
        self.wake.now();
    }

    /// Whether the agent waits on the person to answer a question.
    pub(crate) fn blocks_on_person(&self, agent_id: &str) -> bool {
        self.state.locked().open.iter().any(|question| {
            question.agent_id == agent_id && question.blocked && question.asks_person()
        })
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

    /// The questions put to the person, oldest first.
    pub(crate) fn for_person(&self) -> Vec<ForPerson> {
        self.state
            .locked()
            .open
            .iter()
            .filter(|question| question.asks_person())
            .map(|question| ForPerson {
                id: question.id.clone(),
                agent_id: question.agent_id.clone(),
                text: question.text.clone(),
                blocked: question.blocked,
                assumption: question.assumption.clone(),
                at: question.at,
            })
            .collect()
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

    /// The lead answered the question `id` its agent asked it.
    pub(crate) fn lead_answered(&self, lead: &str, agent_id: &str, id: &str) {
        let mut state = self.state.locked();
        let before = state.open.len();
        state.open.retain(|question| {
            !(question.id == id && question.agent_id == agent_id && question.went_to(lead))
        });
        if state.open.len() != before {
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

/// The agent's turn ended, so a question it was handed and did not answer
/// goes on to the person.
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
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) if !text.trim().is_empty() && text.len() <= max => {
            Ok(Some(text.trim().to_owned()))
        }
        Some(Value::String(text)) if !text.trim().is_empty() => Err(format!(
            "{key} is {} bytes, more than the {max} it may hold",
            text.len()
        )),
        Some(_) => Err(format!("{key} must be nonempty text")),
    }
}

fn choices(value: Option<&Value>) -> Result<Vec<Choice>, String> {
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
            if label.trim().is_empty() {
                return Err("each choice needs a label".to_owned());
            }
            Ok(Choice {
                label: clipped(label, MAX_LABEL_CHARS),
                detail: detail
                    .filter(|detail| !detail.trim().is_empty())
                    .map(|detail| clipped(detail, MAX_DETAIL_CHARS)),
            })
        })
        .collect()
}

fn running(core: &Core, agent_id: &str) -> bool {
    core.chats.get(agent_id).is_some()
        || core.agent_session(agent_id).is_some()
        || core.hosts.team.get(agent_id).is_some()
}

/// Whether the person closed the agent, so its questions go with it.
fn gone(core: &Core, agent_id: &str) -> bool {
    !running(core, agent_id) && core.workspaces.closed(agent_id)
}

/// Whether a message reaches the agent now: it runs, or it sleeps in an open
/// window that wakes it. The window may not have said yet which chats it holds.
fn reachable(core: &Core, agent_id: &str) -> bool {
    running(core, agent_id)
        || (core.window.is_open()
            && !core.workspaces.closed(agent_id)
            && core.workspaces.published(agent_id) != Some(false))
}

fn busy(core: &Core, agent_id: &str) -> bool {
    if let Some(chat) = core.chats.get(agent_id) {
        return chat.turn_running();
    }
    if let Some(foreign) = core.hosts.team.get(agent_id) {
        return foreign.status == crate::protocol::AgentStatus::Working;
    }
    core.agent_session(agent_id)
        .and_then(|session| session.agent.as_ref().map(|agent| !agent.idle()))
        .unwrap_or(false)
}

fn cli_of(core: &Core, agent_id: &str) -> String {
    core.chats
        .get(agent_id)
        .map(|chat| chat.provider().to_owned())
        .or_else(|| {
            core.agent_session(agent_id)
                .and_then(|session| session.info().agent_type)
        })
        .unwrap_or_else(|| "agent".to_owned())
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
    let lead = named
        .as_ref()
        .and_then(|named| named.parent.clone())
        .filter(|lead| reachable(core, lead));
    let stage = match lead {
        Some(lead_id) => Stage::Lead {
            lead_name: core.names.name(&lead_id).unwrap_or_else(|| lead_id.clone()),
            lead_id,
            ended_at: None,
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
    let project = named.map(|named| named.project).unwrap_or_else(|| {
        project_of(
            std::path::Path::new(project),
            &core.workspaces.project_paths(),
        )
    });
    let cli = cli_of(core, agent_id);
    let at = epoch_ms();
    let questions: Vec<Question> = asked
        .into_iter()
        .map(|asked| Question {
            stage: if asked.secret {
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
            format!("Go on assuming {assumption}. A message tells you if that is wrong.")
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
            format!("Sent to {lead_name}, your lead, which answers it or asks the person.")
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
            "\n\n{name} goes on assuming {assumption} until it hears otherwise."
        )),
        None => text.push_str(&format!(
            "\n\n{name} waits for the answer before it goes on."
        )),
    }
    let note = format!(
        "{name}, an agent you started, asks you this. Answer it with agent_send to {name}, setting answers to {}. If you do not know, pass it to the person with ask, setting for to {name}. If your turn ends with neither, Sikemux asks the person.",
        question.id
    );
    (text, note)
}

/// Hands a question that went to the asker's lead to it, or to the person
/// when the lead cannot take it.
async fn hand_to_lead(core: &Arc<Core>, question: &Question) {
    let Some(lead_id) = question.lead_id().map(str::to_owned) else {
        return;
    };
    let (text, note) = for_lead(question);
    if messages::question(core, question.agent_id.clone(), lead_id, text, note)
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

pub(crate) async fn ask(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    let agent_id = request
        .agent_id
        .clone()
        .ok_or("ask needs the calling agent's id")?;
    let params = &request.params;
    let text = text_param(params, "question", MAX_QUESTION_BYTES)?
        .ok_or("question must be what you want to know, as nonempty text")?;
    let choices = choices(params.get("choices"))?;
    let header = text_param(params, "header", MAX_QUESTION_BYTES)?
        .map(|header| clipped(&header, MAX_HEADER_CHARS));
    if let Some(worker) = text_param(params, "for", 200)? {
        if let Some(foreign) = foreign_worker(core, &agent_id, &worker) {
            let choices = serde_json::to_value(&choices).map_err(|error| error.to_string())?;
            return super::hosts::route::pass_on(core, &agent_id, &foreign, text, choices).await;
        }
        return pass_on(core, &agent_id, &worker, text, choices);
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
            Stage::Person { .. } => "the person".to_owned(),
        },
        "note": sent_note(&question),
    }))
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
pub(crate) fn pass_on_from(core: &Core, lead: &str, params: &Value) -> Result<Value, String> {
    let worker = params["worker"].as_str().unwrap_or_default();
    let text = text_param(params, "text", MAX_QUESTION_BYTES)?
        .ok_or("the question must be nonempty text")?;
    let choices = choices(params.get("choices"))?;
    pass_on(core, lead, worker, text, choices)
}

/// A lead passes a question its agent asked it on to the person.
fn pass_on(
    core: &Core,
    lead_id: &str,
    worker: &str,
    text: String,
    choices: Vec<Choice>,
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
            })
        })
        .collect();
    if asked.is_empty() {
        return "Sikemux could not read that question; ask it again with the ask tool.".into();
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

/// The open questions, for the app, the notch and phones.
pub(crate) fn views(core: &Core) -> Vec<Value> {
    let open = core.harness.questions.state.locked().open.clone();
    with_tickets(core, open)
}

/// Each question's view, with the ticket of the card its asker holds.
fn with_tickets(core: &Core, open: Vec<Question>) -> Vec<Value> {
    open.iter()
        .map(|question| {
            let mut view = question.view();
            if let (Some(ticket), Some(fields)) = (
                super::board::ticket_of_agent(core, &question.agent_id),
                view.as_object_mut(),
            ) {
                fields.insert("ticket".into(), ticket.into());
            }
            view
        })
        .collect()
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AnswerParams {
    id: String,
    answer: Option<String>,
    choice: Option<usize>,
    /// The choices picked on a question that takes several.
    #[serde(skip_serializing_if = "Option::is_none")]
    choices: Option<Vec<usize>>,
    /// The assumption the agent went on is right.
    #[serde(default)]
    fine: bool,
    /// The person skipped the question rather than answer it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    skip: bool,
}

/// The person's answer: the first one wins, and reaches the asker as a
/// message.
pub(crate) async fn answer(core: &Arc<Core>, params: AnswerParams) -> Result<Value, String> {
    answer_all(
        core,
        AnswersParams {
            answers: vec![params],
        },
    )
    .await
}

#[derive(Deserialize)]
pub(crate) struct AnswersParams {
    answers: Vec<AnswerParams>,
}

/// The person's answers to questions asked together. Each asker gets all of
/// its answers in one message, so the first does not start a turn before the
/// rest arrive.
pub(crate) async fn answer_all(core: &Arc<Core>, params: AnswersParams) -> Result<Value, String> {
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
        core.harness.questions.changed(&mut state);
        taken
    };
    let reopen = |questions: Vec<Question>| {
        let mut state = core.harness.questions.state.locked();
        state.open.extend(questions);
        core.harness.questions.changed(&mut state);
    };
    let mut letters: Vec<(Question, String, bool)> = Vec::new();
    for (question, given, text) in taken {
        if let Some(host) = &question.from_host {
            let params = serde_json::to_value(&given).map_err(|error| error.to_string())?;
            super::hosts::route::relay_answer(core, host, params);
            continue;
        }
        let Some(mut text) = text else { continue };
        if question.secret && !given.skip {
            match core.harness.questions.keep_secret(&question, &text) {
                Ok(path) => text = secret_note(&path),
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
        letters.push((question, text, given.skip));
    }
    let mut askers: Vec<&str> = letters
        .iter()
        .map(|(question, ..)| question.agent_id.as_str())
        .collect();
    askers.sort_unstable();
    askers.dedup();
    let mut failed = None;
    for asker in askers {
        let own: Vec<&(Question, String, bool)> = letters
            .iter()
            .filter(|(question, ..)| question.agent_id == asker)
            .collect();
        let sent = own
            .iter()
            .map(|(question, text, skipped)| {
                let note = match (&question.assumption, skipped) {
                    (_, true) => "The person skipped your question rather than answer it. Do not go ahead with what you asked it for, and do not ask it again: say in a line that you have stopped, and wait for their next message.".to_owned(),
                    (Some(assumption), false) => format!(
                        "The person answered the question you went on without. If that is not what you assumed ({assumption}), change course."
                    ),
                    (None, false) => "The person answered the question you asked; go on with it.".to_owned(),
                };
                (answered(question, &question.asked, *skipped), text.clone(), note)
            })
            .collect();
        if let Err(error) = messages::answer(core, asker.to_owned(), sent).await {
            reopen(
                own.into_iter()
                    .map(|(question, ..)| question.clone())
                    .collect(),
            );
            failed = Some(error);
            continue;
        }
        for (question, text, skipped) in own {
            copy_to_lead(core, question, text, *skipped).await;
        }
    }
    match failed {
        Some(error) => Err(error),
        None => Ok(json!({ "answered": true })),
    }
}

/// The person's answer, copied to the lead a question went to first.
async fn copy_to_lead(core: &Arc<Core>, question: &Question, text: &str, skipped: bool) {
    if let (
        false,
        Stage::Person {
            lead: Some((lead_id, _)),
            ..
        },
    ) = (question.secret, &question.stage)
    {
        let name = &question.name;
        let note = if skipped {
            format!("The person skipped {name}'s question, which went to you first, rather than answer it. {name} has stopped to wait for them.")
        } else {
            format!("The person answered {name}'s question, which went to you first. {name} has the answer too.")
        };
        let _ = messages::answer(
            core,
            lead_id.clone(),
            vec![(
                answered(question, &question.text, skipped),
                text.to_owned(),
                note,
            )],
        )
        .await;
    }
}

fn answered(question: &Question, question_text: &str, skipped: bool) -> messages::Answered {
    messages::Answered {
        question: question_text.to_owned(),
        asker: question.name.clone(),
        cli: question.cli.clone(),
        secret: question.secret && !skipped,
        skipped,
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
    if params.skip {
        return Ok(Some(SKIPPED.to_owned()));
    }
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
    match params.answer.as_deref().map(str::trim) {
        Some(text) if !text.is_empty() && text.len() <= MAX_ANSWER_BYTES => {
            Ok(Some(text.to_owned()))
        }
        Some(text) if !text.is_empty() => Err(format!(
            "An answer holds at most {} KB",
            MAX_ANSWER_BYTES / 1024
        )),
        _ => Err("The answer is empty".into()),
    }
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
                .map(|question| question.agent_id.clone())
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
            let state = (reachable(core, &lead), busy(core, &lead));
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
        let mut changed = state.open.len() != before;
        for question in &mut state.open {
            let Stage::Lead {
                lead_id,
                lead_name,
                ended_at,
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
        if changed {
            core.harness.questions.changed(&mut state);
            core.boards.wake.now();
        }
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

/// Puts each question now for the person, whose asker's team is led from a
/// linked host, to the person there instead.
fn relay(core: &Core) {
    let candidates: Vec<String> = core
        .harness
        .questions
        .state
        .locked()
        .open
        .iter()
        .filter(|question| {
            matches!(question.stage, Stage::Person { .. })
                && question.relayed.is_none()
                && question.from_host.is_none()
        })
        .map(|question| question.agent_id.clone())
        .collect();
    if candidates.is_empty() {
        return;
    }
    let hosts: HashMap<String, String> = candidates
        .into_iter()
        .filter_map(|agent_id| {
            let host = super::hosts::route::lead_host(core, &agent_id)?;
            Some((agent_id, host))
        })
        .collect();
    if hosts.is_empty() {
        return;
    }
    let mut relayed = Vec::new();
    {
        let mut state = core.harness.questions.state.locked();
        for question in &mut state.open {
            let Some(host) = hosts.get(&question.agent_id) else {
                continue;
            };
            if !matches!(question.stage, Stage::Person { .. }) || question.relayed.is_some() {
                continue;
            }
            question.relayed = Some(host.clone());
            relayed.push((host.clone(), question.clone()));
        }
        if !relayed.is_empty() {
            core.harness.questions.changed(&mut state);
        }
    }
    for (host, question) in relayed {
        super::hosts::route::relay_question(
            core,
            &host,
            serde_json::to_value(&question).unwrap_or_default(),
        );
    }
}

/// A question an agent on a linked host asked, which waits on the person
/// here, where its team is led from.
pub(crate) fn relayed_here(core: &Core, host: &str, params: Value) -> Result<Value, String> {
    let mut question: Question =
        serde_json::from_value(params).map_err(|error| error.to_string())?;
    let foreign = core
        .hosts
        .team
        .get(&question.agent_id)
        .filter(|foreign| foreign.host == host)
        .ok_or("that agent is not one of a team this host shares with yours")?;
    if question.text.len() > MAX_QUESTION_BYTES || question.choices.len() > MAX_CHOICES {
        return Err("that question is larger than this host takes".into());
    }
    question.name = format!("{} on {}", foreign.name, super::hosts::name_of(core, host));
    question.project = foreign.project;
    question.relayed = None;
    question.from_host = Some(host.to_owned());
    question.stage = match question.stage {
        Stage::Person { lead, passed } => Stage::Person { lead, passed },
        Stage::Lead { .. } => Stage::Person {
            lead: None,
            passed: false,
        },
    };
    let mut state = core.harness.questions.state.locked();
    state.open.retain(|open| open.id != question.id);
    state.open.push(question);
    core.harness.questions.changed(&mut state);
    Ok(Value::Null)
}

/// The person on the linked host this question was put to answered it.
pub(crate) async fn answer_relayed(
    core: &Arc<Core>,
    host: &str,
    params: Value,
) -> Result<Value, String> {
    let params: AnswerParams = serde_json::from_value(params).map_err(|error| error.to_string())?;
    let relayed_there = core
        .harness
        .questions
        .state
        .locked()
        .open
        .iter()
        .any(|question| question.id == params.id && question.relayed.as_deref() == Some(host));
    if !relayed_there {
        return Ok(json!({ "answered": false }));
    }
    answer(core, params).await
}

/// A question an agent on a linked host asked no longer waits on the person.
pub(crate) fn closed_elsewhere(core: &Core, host: &str, params: Value) -> Result<Value, String> {
    let id = params["id"].as_str().unwrap_or_default();
    let mut state = core.harness.questions.state.locked();
    let before = state.open.len();
    state
        .open
        .retain(|question| !(question.id == id && question.from_host.as_deref() == Some(host)));
    if state.open.len() != before {
        core.harness.questions.changed(&mut state);
    }
    Ok(Value::Null)
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
        let now: HashMap<String, Question> = state
            .open
            .iter()
            .filter(|question| question.blocked && question.asks_person())
            .map(|question| (question.id.clone(), question.clone()))
            .collect();
        let asked: Vec<Question> = now
            .values()
            .filter(|question| !state.told_phones.contains_key(&question.id))
            .cloned()
            .collect();
        let cleared: Vec<(String, String)> = state
            .told_phones
            .iter()
            .filter(|(id, _)| !now.contains_key(*id))
            .map(|(id, agent_id)| (id.clone(), agent_id.clone()))
            .collect();
        state.told_phones = now
            .into_iter()
            .map(|(id, question)| (id, question.agent_id))
            .collect();
        (asked, cleared, changed)
    };
    for agent_id in changed {
        core.seen.active(&agent_id);
    }
    for question in asked {
        core.note_activity(&question.agent_id, Activity::NeedsYou);
        core.notify.send(Signal::Question {
            id: question.id,
            agent_id: question.agent_id,
            name: question.name,
            text: question.text,
            at: question.at,
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
    let open = {
        let mut state = core.harness.questions.state.locked();
        if !state.dirty && state.told_window == window {
            return;
        }
        if state.dirty {
            core.view_changed();
        }
        state.dirty = false;
        state.told_window = window;
        if window.is_none() {
            return;
        }
        state.open.clone()
    };
    let views = with_tickets(core, open);
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
    use super::*;

    fn question(secret: bool) -> Question {
        serde_json::from_value(json!({
            "id": "q1", "agentId": "a1", "name": "Maple", "cli": "claude", "project": "/p",
            "asked": "Ship it?", "text": "Ship it?", "choices": [{ "label": "Yes" }],
            "blocked": true, "assumption": null, "secret": secret, "at": 0,
            "stage": { "stage": "person", "lead": null, "passed": false }
        }))
        .expect("question")
    }

    fn params(value: Value) -> AnswerParams {
        serde_json::from_value(value).expect("answer params")
    }

    #[test]
    fn a_skipped_question_needs_no_answer_and_tells_the_agent_so() {
        let skipped = params(json!({ "id": "q1", "skip": true }));
        assert_eq!(
            answer_text(&question(false), &skipped),
            Ok(Some(SKIPPED.to_owned()))
        );
        assert_eq!(
            answer_text(&question(true), &skipped),
            Ok(Some(SKIPPED.to_owned()))
        );
        let empty = params(json!({ "id": "q1" }));
        assert!(answer_text(&question(false), &empty).is_err());
    }

    #[test]
    fn a_skipped_secret_is_shown_as_skipped_rather_than_kept_private() {
        let letter = answered(&question(true), "Ship it?", true);
        assert!(letter.skipped);
        assert!(!letter.secret);
        let answered_secret = answered(&question(true), "Ship it?", false);
        assert!(answered_secret.secret);
        assert!(!answered_secret.skipped);
    }

    #[test]
    fn skip_crosses_to_a_linked_host_only_when_set() {
        let skipped =
            serde_json::to_value(params(json!({ "id": "q1", "skip": true }))).expect("json");
        assert_eq!(skipped["skip"], true);
        let picked =
            serde_json::to_value(params(json!({ "id": "q1", "choice": 0 }))).expect("json");
        assert!(picked.get("skip").is_none());
    }
}
