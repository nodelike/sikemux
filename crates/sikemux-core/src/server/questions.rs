//! What `ask` answers, and the CLIs' own question tools with it. An agent's
//! question goes first to the agent that started it, which decides. A lead that
//! ends its turn without answering leaves an assumption standing, and is handed
//! a question the asker waits on back once before it reaches the person. Only
//! what a lead raises itself, or what an agent with no lead asks, reaches them.
//! The person answers in the asker's chat, the notch or Sikemux's rail, and the
//! first answer reaches the asker as a message.

use crate::server::activity::ActivityCore;
use crate::server::agent_now::AgentNowCore;
use crate::server::changed::ChangedCore;
use crate::server::changed::What;
use crate::server::parts::Parts;
use crate::server::serving::ServeParts;
pub use sikemux_calls::questions::{AnswerParams, AnswersParams};
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
use sikemux_view::calls::{
    CallAsker, CallBrief, CallEvidence, CallKind, CallOption, CallState, PersonCall,
};
use sikemux_view::inbox::{InboxRaised, RaisedFor};
use sikemux_view::questions::{AgentQuestion, QuestionChoice, QuestionState};
use sikemux_wire::protocol::WindowCall;

use super::activity::Activity;
use super::approvals::{Approval, Keeper};
use super::changed::Topic;
use super::chat::ChatCommand;
use super::connection::ClientId;
use super::kept::Kept;
use super::messages;
use super::notify::Signal;
use super::person_calls;
use super::wake::{sooner, Wake};
use super::Core;

mod relay;
mod reply;

use relay::*;
pub(crate) use relay::{
    answer_relayed, answer_turned_away, closed_elsewhere, relay_arrived, relayed_here,
};
use reply::*;

const FILE: &str = "agent-questions.json";
/// Where the person's answers to secret questions wait for their agents.
const SECRETS: &str = "agent-secrets";
/// Where copies of the pictures choices show are kept, so they outlive the files agents made.
const PICTURES: &str = "question-pictures";
const PICTURE_TYPES: [&str; 5] = ["png", "jpg", "jpeg", "gif", "webp"];
const MAX_PICTURE_BYTES: u64 = 20 * 1024 * 1024;
/// The answers to secret questions kept for one agent, newest first.
const MAX_SECRETS_PER_AGENT: usize = 8;
const MAX_QUESTION_BYTES: usize = 8 * 1024;
const MAX_CHOICES: usize = 9;
const MAX_LABEL_CHARS: usize = 200;
const MAX_DETAIL_CHARS: usize = 400;
/// What the person needs to know to decide, beside the question.
const MAX_CONTEXT_CHARS: usize = 600;
/// The most of what an asker goes on with that its call shows, on one line.
const MAX_ASSUMPTION_CHARS: usize = 120;
/// The most a raised question shows to look at.
const MAX_EVIDENCE: usize = 4;
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
        /// The person handed it back to this lead, so it never reaches them again.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        left_by_person: bool,
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
    /// What the person asked for, in their words, which heads the call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    asked_for: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    context: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    evidence: Vec<CallEvidence>,
    /// What a weighty call says first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    brief: Option<CallBrief>,
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
    /// The linked host it was also put to the person on, where its asker's team
    /// is led from.
    #[serde(skip_serializing_if = "Option::is_none")]
    relayed: Option<String>,
    /// The host it was relayed to took it, so that host lists it for phones and this one does not.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    relay_taken: bool,
    /// Why the answer the person gave it here never reached its asker on another host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    undelivered: Option<String>,
    /// The linked host its asker runs on, when it is put to the person here
    /// for an agent there.
    #[serde(skip_serializing_if = "Option::is_none")]
    from_host: Option<String>,
    /// The plugin tool call the person approves or turns down with their
    /// answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    approval: Option<Approval>,
    /// The slice whose proof the answer posts to its pull request, or keeps off it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    proof: Option<ProofAsk>,
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
            via_note: self.undelivered.clone().or(via_note),
            secret_note: self.secret.then(|| format!(
                "Kept private. {} reads it from a file only your account can open, and it never shows in the chat.",
                self.name
            )),
            answer: None,
            ticket: None,
            production: self.production_call(),
        }
    }

    /// Its call to the person is one for a change to production.
    fn production_call(&self) -> bool {
        !self.secret && (self.approval.is_some() || self.production())
    }

    /// It changes a production place, so only the person's tap on its call answers it: never its
    /// lead, and never words typed in the chat.
    fn production(&self) -> bool {
        (self.raise.as_ref()).is_some_and(|raise| raise.reason == RaisedFor::Irreversible)
    }

    /// Its answer may be a password or a token, so it is never shown again.
    fn hides_answer(&self) -> bool {
        self.secret || (self.raise.as_ref()).is_some_and(|raise| raise.reason == RaisedFor::Access)
    }

    /// The option of its call that runs what it holds, on an approval of a plugin call.
    /// The option of its call that lets what it is about run: an approval's Approve, or for a
    /// change to a production place, the asker's pick.
    fn allows(&self) -> Option<String> {
        if let Some(approval) = &self.approval {
            return approval
                .approving()
                .map(|approving| (approving + 1).to_string());
        }
        let pick = self.choices.iter().position(|choice| choice.recommended)?;
        self.production().then(|| (pick + 1).to_string())
    }

    /// It as a call to the person.
    fn call(&self) -> PersonCall {
        let reason = self.raise.as_ref().map(|raise| raise.reason);
        let kind = match reason {
            _ if self.secret => CallKind::Access,
            _ if self.approval.is_some() || self.production() => CallKind::Production,
            Some(RaisedFor::Money) => CallKind::Money,
            Some(RaisedFor::Access) => CallKind::Access,
            Some(RaisedFor::Taste | RaisedFor::Irreversible) | None => CallKind::Taste,
        };
        let only_theirs = self.approval.is_some()
            || self.proof.is_some()
            || self.dialog.is_some()
            || self.from_host.is_some();
        let breaks = self.raise.as_ref().map(|raise| raise.breaks.clone());
        let given = self.raise.as_ref().and_then(|raise| raise.context.clone());
        let (context, against) = match kind {
            CallKind::Access => (given.or(breaks), None),
            _ => (given, breaks),
        };
        let assumption = (kind != CallKind::Production)
            .then_some(self.assumption.as_deref())
            .flatten()
            .map(|assumption| clipped(&crate::text::one_line(assumption), MAX_ASSUMPTION_CHARS));
        let (options, pick) = call_options(&self.choices);
        PersonCall {
            id: self.id.clone(),
            project: self.project.clone(),
            kind,
            asker: CallAsker {
                agent_id: self.agent_id.clone(),
                name: self.name.clone(),
                cli: self.cli.clone(),
                chief: false,
            },
            joined: Vec::new(),
            asked_for: self
                .raise
                .as_ref()
                .and_then(|raise| raise.asked_for.clone()),
            question: self.text.clone(),
            context,
            assumption,
            options,
            pick,
            against,
            evidence: (self.raise.as_ref()).map_or_else(Vec::new, |raise| raise.evidence.clone()),
            action: None,
            brief: self.raise.as_ref().and_then(|raise| raise.brief.clone()),
            place: None,
            can_hand_back: kind == CallKind::Taste && !only_theirs,
            multi: self.multi,
            secret: self.secret,
            raised_at: self.at,
            state: CallState::Open,
            answer: None,
            outcome: None,
        }
    }

    /// Whether this host lists it where every host's questions are gathered, as phones and the
    /// inbox do: a question another host took to put to the person is listed there alone.
    fn listed_for_devices(&self) -> bool {
        self.asks_person() && !(self.relayed.is_some() && self.relay_taken)
    }

    fn asks_person(&self) -> bool {
        matches!(self.stage, Stage::Person { .. }) && !self.stands()
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

#[derive(Default)]
struct State {
    open: Vec<Question>,
    /// The rule each agent was last sent back to over what it offered, so the same rule does not
    /// send it back again until a turn ends offering nothing a rule is about.
    followed_rules: HashMap<String, String>,
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
    /// The open questions also put to the person on another host, and which.
    relayed_to: HashMap<String, String>,
}

pub(crate) struct Questions {
    state: Mutex<State>,
    kept: Kept,
    secrets: Option<std::path::PathBuf>,
    pictures: Option<std::path::PathBuf>,
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
    /// The picture each of `answers` shows, when its asker gave them.
    pub pictures: Vec<Option<String>>,
    pub at: u64,
    pub raised: Option<InboxRaised>,
    /// The one of `answers` its asker would take.
    pub pick: Option<String>,
}

/// A question's choices as a call's options, each named by its place, and the one marked as the pick.
fn call_options(choices: &[QuestionChoice]) -> (Vec<CallOption>, Option<String>) {
    let id = |index: usize| (index + 1).to_string();
    let options = (choices.iter().enumerate())
        .map(|(index, choice)| CallOption {
            id: id(index),
            label: choice.label.clone(),
            detail: choice.detail.clone(),
            picture: choice.picture.clone(),
        })
        .collect();
    let pick = choices.iter().position(|choice| choice.recommended).map(id);
    (options, pick)
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
    proof: Option<ProofAsk>,
    dialog: Option<String>,
    raise: Option<Raise>,
}

/// A slice of the board at `project`, by its card.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProofAsk {
    pub project: String,
    pub card: u64,
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
        let pictures = data_dir.map(|dir| dir.join(PICTURES));
        if let Some(entries) = pictures
            .as_deref()
            .and_then(|dir| std::fs::read_dir(dir).ok())
        {
            let chosen = (record.open.iter())
                .flat_map(|question| question.choices.iter())
                .filter_map(|choice| choice.picture.clone());
            let evidence = (record.open.iter())
                .filter_map(|question| question.raise.as_ref())
                .flat_map(|raise| raise.evidence.iter())
                .filter_map(|evidence| evidence.picture.clone());
            let shown: HashSet<String> = chosen.chain(evidence).collect();
            for entry in entries.flatten() {
                if !shown.contains(&*entry.path().to_string_lossy()) {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
        Self {
            secrets,
            pictures,
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

    /// Copies the pictures of the raise's evidence, as [`Self::keep_pictures`] copies choices'.
    fn keep_evidence(&self, folder: &str, raise: &mut Option<Raise>) -> Result<(), String> {
        let Some(raise) = raise.as_mut() else {
            return Ok(());
        };
        let mut shown: Vec<QuestionChoice> = (raise.evidence.iter())
            .map(|evidence| QuestionChoice {
                label: evidence.what.clone(),
                detail: None,
                picture: evidence.picture.clone(),
                recommended: false,
            })
            .collect();
        self.keep_pictures(folder, &mut shown)?;
        for (evidence, kept) in raise.evidence.iter_mut().zip(shown) {
            evidence.picture = kept.picture;
        }
        Ok(())
    }

    /// Whether `path` is one of the pictures questions show, which any paired device may open.
    pub(crate) fn keeps_picture(&self, path: &Path) -> bool {
        (self.pictures.as_deref()).is_some_and(|dir| super::attachments::is_kept(dir, path))
    }

    /// Copies the picture each choice names, a path from `folder`, into the core's own folder,
    /// pointing the choice at the copy. Nothing is copied unless every picture can be.
    fn keep_pictures(&self, folder: &str, choices: &mut [QuestionChoice]) -> Result<(), String> {
        let given: Vec<std::path::PathBuf> = (choices.iter())
            .filter_map(|choice| choice.picture.as_deref())
            .map(|picture| picture_file(folder, picture))
            .collect::<Result<_, _>>()?;
        if given.is_empty() {
            return Ok(());
        }
        let dir = (self.pictures.as_ref()).ok_or("Sikemux has nowhere to keep pictures")?;
        std::fs::create_dir_all(dir)
            .map_err(|error| format!("Sikemux could not keep the pictures: {error}"))?;
        let shown = choices.iter_mut().filter(|choice| choice.picture.is_some());
        for (choice, from) in shown.zip(given) {
            let extension = (from.extension()).map_or(String::new(), |extension| {
                extension.to_string_lossy().to_lowercase()
            });
            let to = dir.join(format!("{}.{extension}", uuid::Uuid::new_v4().simple()));
            std::fs::copy(&from, &to)
                .map_err(|error| format!("Sikemux could not keep {}: {error}", from.display()))?;
            choice.picture = Some(to.to_string_lossy().into_owned());
        }
        Ok(())
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
        state.dirty = true;
        self.wake.now();
    }

    /// The questions of `agent_id` the person answers by writing to it, which its next turn takes
    /// back: those it stopped for.
    pub(crate) fn answered_by_writing(&self, agent_id: &str) -> Vec<String> {
        (self.state.locked().open.iter())
            .filter(|question| {
                question.agent_id == agent_id
                    && question.blocked
                    && question.asks_person()
                    && question.approval.is_none()
            })
            .map(|question| question.id.clone())
            .collect()
    }

    /// Whether what the person types to `agent_id` may answer an open question whose answer is
    /// never kept: one it asked, or one a worker of its asked that went to it first. A question
    /// a linked host took to put to the person there has no call here, and counts all the same.
    pub(crate) fn hears_hidden(&self, agent_id: &str) -> bool {
        (self.state.locked().open.iter()).any(|question| {
            question.hides_answer() && (question.agent_id == agent_id || question.went_to(agent_id))
        })
    }

    pub(crate) fn is_open(&self, id: &str) -> bool {
        (self.state.locked().open.iter()).any(|question| question.id == id)
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

    /// The questions put to the person, oldest first: of those an agent waits on, only its
    /// latest.
    pub(crate) fn for_person(&self) -> Vec<ForPerson> {
        let state = self.state.locked();
        let asked = (state.open.iter())
            .filter(|question| question.listed_for_devices())
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
                pictures: match question.multi
                    || question
                        .choices
                        .iter()
                        .all(|choice| choice.picture.is_none())
                {
                    true => Vec::new(),
                    false => (question.choices.iter())
                        .map(|choice| choice.picture.clone())
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
        let questions: Vec<ForPerson> = asked.collect();
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

    /// The agent began another turn, as when the person wrote to it. A question it stopped for
    /// no longer waits, unless the call it raised is still `open`: a call waits until the person
    /// answers it. An assumption it went on with, and a plugin call waiting for the person's
    /// approval, still wait.
    pub(crate) fn turn_started(&self, agent_id: &str, open: &[String]) {
        let mut state = self.state.locked();
        let before = state.open.len();
        state.open.retain(|question| {
            let waited_on = question.blocked && question.asks_person();
            question.agent_id != agent_id
                || !waited_on
                || question.approval.is_some()
                || open.contains(&question.id)
        });
        if state.open.len() != before {
            self.changed(&mut state);
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
                question.id == id
                    && question.agent_id == agent_id
                    && question.went_to(lead)
                    && !question.production()
            })
            .map(|question| question.asked.clone())
    }

    /// Closes the question `id` its agent asked `lead`, which answered it, while it is open.
    fn take_answered(&self, lead: &str, agent_id: &str, id: &str) -> Option<Question> {
        let mut state = self.state.locked();
        let at = (state.open.iter()).position(|question| {
            question.id == id
                && question.agent_id == agent_id
                && question.went_to(lead)
                && !question.production()
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
    let Some(question) = core.harness().questions.take_answered(lead, agent_id, id) else {
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
    let questions = &core.harness().questions;
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
            let (label, detail, picture) = match choice {
                Value::String(label) => (label.as_str(), None, None),
                Value::Object(choice) => (
                    choice
                        .get("label")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                    choice
                        .get("detail")
                        .or_else(|| choice.get("description"))
                        .and_then(Value::as_str),
                    choice.get("picture").and_then(Value::as_str),
                ),
                _ => ("", None, None),
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
                picture: (picture.map(str::trim))
                    .filter(|picture| !picture.is_empty())
                    .map(str::to_owned),
                recommended,
            })
        })
        .collect()
}

/// The image file a choice's `picture` names, from `folder` when it is not a whole path.
fn picture_file(folder: &str, picture: &str) -> Result<std::path::PathBuf, String> {
    let path = Path::new(folder).join(picture);
    let extension = (path.extension()).map(|extension| extension.to_string_lossy().to_lowercase());
    if !extension.is_some_and(|extension| PICTURE_TYPES.contains(&extension.as_str())) {
        return Err(format!(
            "picture {picture} is not a PNG, JPEG, GIF or WebP file"
        ));
    }
    match std::fs::metadata(&path) {
        Ok(file) if file.is_file() && file.len() <= MAX_PICTURE_BYTES => Ok(path),
        Ok(file) if file.is_file() => Err(format!("picture {picture} is over 20 MB")),
        _ => Err(format!("picture {picture} is not a file Sikemux can read")),
    }
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
    let bare: Vec<&str> = (choices.iter())
        .filter(|choice| choice.picture.is_none())
        .map(|choice| choice.label.as_str())
        .collect();
    if reason == Some(RaisedFor::Taste) && !choices.is_empty() && !bare.is_empty() {
        missing.push(format!(
            "picture on each choice: the path of a screenshot or mockup that shows it, missing from {}",
            bare.join(", ")
        ));
    }
    if breaks.is_none() {
        missing.push("breaks: what breaks if that is wrong".to_owned());
    }
    let context = text_param(params, "context", MAX_CONTEXT_CHARS)?;
    let evidence = evidence_of(params.get("evidence"))?;
    let brief = brief_of(params.get("brief"))?;
    match (reason, breaks) {
        (Some(reason), Some(breaks)) if missing.is_empty() => Ok(Raise {
            reason,
            pick,
            breaks,
            asked_for: None,
            context,
            evidence,
            brief,
        }),
        _ => Err(format!(
            "A decision raised to the person has to stand alone, and this one is missing:\n- {}\nIf none of {REASONS} is the reason, decide it yourself.",
            missing.join("\n- ")
        )),
    }
}

/// What the person asked `agent` for, in their own words, which heads a call it raises: the
/// prompt of theirs that started its turn, as the core recorded it, cut to the card's heading.
/// None when the person's words did not start the turn, answered a question, may hold a secret,
/// or are marked as answering a call or as typed while one whose words are never kept was open.
/// Reads the chat's history file, so it blocks.
fn asked_for(core: &Core, agent: &str) -> Option<String> {
    let chat = core.chats().get(agent)?;
    let dir = core.chats().history_dir()?;
    let said = super::chat::history::turn_prompt(dir, chat.feed.id(), MAX_ASKED_FOR_BYTES)?;
    if said.answered.is_some() || said.text.trim_start().starts_with("<question-answer ") {
        return None;
    }
    let words = crate::sessions::asked::person_prose(&said.text)?;
    (!crate::sessions::asked::may_hold_secret(&words))
        .then(|| clipped(&crate::text::one_line(&words), MAX_LABEL_CHARS))
}

/// How much of a chat's history is read for the prompt that started its turn.
const MAX_ASKED_FOR_BYTES: u64 = 512 * 1024;

/// What a raised question shows to look at before deciding.
fn evidence_of(value: Option<&Value>) -> Result<Vec<CallEvidence>, String> {
    let list = match value {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(list)) => list,
        Some(_) => return Err("evidence must be a list".into()),
    };
    if list.len() > MAX_EVIDENCE {
        return Err(format!(
            "evidence takes at most {MAX_EVIDENCE} things to look at"
        ));
    }
    (list.iter())
        .map(|item| {
            let what = text_param(item, "what", MAX_DETAIL_CHARS)?
                .ok_or("each piece of evidence needs what: what it shows, in words")?;
            let picture = text_param(item, "picture", 4096)?;
            Ok(CallEvidence { what, picture })
        })
        .collect()
}

/// The brief of a weighty decision, which needs all three of its parts.
fn brief_of(value: Option<&Value>) -> Result<Option<CallBrief>, String> {
    let brief = match value {
        None | Some(Value::Null) => return Ok(None),
        Some(brief @ Value::Object(_)) => brief,
        Some(_) => return Err("brief must be an object".into()),
    };
    let part = |key: &str| {
        text_param(brief, key, MAX_DETAIL_CHARS)?.ok_or_else(|| {
            format!("brief needs {key}: what was tried, what the evidence shows and what can't be undone")
        })
    };
    Ok(Some(CallBrief {
        tried: part("tried")?,
        shows: part("shows")?,
        cannot_undo: part("cannotUndo")?,
    }))
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
    !core.agent_now(agent_id).running() && core.workspaces().closed(agent_id)
}

/// Whether a message reaches the agent now: it runs, or it sleeps in an open
/// window that wakes it. The window may not have said yet which chats it holds.
fn reachable(core: &Core, agent_id: &str) -> bool {
    core.agent_now(agent_id).running()
        || (core.window().is_open()
            && !core.workspaces().closed(agent_id)
            && core.workspaces().published(agent_id) != Some(false))
}

pub(crate) fn cli_of(core: &Core, agent_id: &str) -> String {
    core.agent_now(agent_id)
        .cli()
        .unwrap_or_else(|| "agent".to_owned())
}

/// The agent that started `agent_id`, while a message reaches it.
fn reachable_lead(core: &Core, agent_id: &str) -> Option<String> {
    let led_by_auto = (core.hosts().team.tree(agent_id)).is_some_and(|tree| tree.auto);
    (core.names().get(agent_id)?.parent).filter(|lead| !led_by_auto && reachable(core, lead))
}

/// Whether the agent answers to nobody but has agents of its own, so what it asks the person is
/// a decision it raises rather than a question from a lone agent.
fn raises(core: &Core, agent_id: &str) -> bool {
    reachable_lead(core, agent_id).is_none() && core.names().team(agent_id).len() > 1
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
    let named = core.names().get(agent_id);
    let stage = match reachable_lead(core, agent_id) {
        Some(lead_id) => Stage::Lead {
            lead_name: core.names().name_or_id(&lead_id),
            lead_id,
            ended_at: None,
            handed_back: false,
            left_by_person: false,
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
        .unwrap_or_else(|| core.workspaces().project_of(std::path::Path::new(project)));
    let cli = cli_of(core, agent_id);
    let at = epoch_ms();
    let questions: Vec<Question> = asked
        .into_iter()
        .map(|asked| Question {
            stage: if asked.secret
                || asked.approval.is_some()
                || asked.proof.is_some()
                || asked.dialog.is_some()
            {
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
            relay_taken: false,
            undelivered: None,
            from_host: None,
            approval: asked.approval,
            proof: asked.proof,
            dialog: asked.dialog,
            raise: asked.raise,
        })
        .collect();
    let mut state = core.harness().questions.state.locked();
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
    core.harness().questions.changed(&mut state);
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
            if let Some(picture) = &choice.picture {
                text.push_str(&format!(" (picture: {picture})"));
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
    let id = &question.agent_id;
    let note = format!(
        "@{name} ({id}), an agent you started, asks you this. You decide it: answer with agent_send to {id}, setting answers to {} and why. {RAISE_RULE}, setting for to {id}. If your turn ends with neither, {if_silent}.",
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
    let id = &question.agent_id;
    let note = format!(
        "You ended your turn without answering @{name} ({id}), which is still waiting. Decide it now: answer with agent_send to {id}, setting answers to {} and why. {RAISE_RULE}, setting for to {id}. If this turn ends with neither, it goes to the person as something you could not decide.",
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
    let mut state = core.harness().questions.state.locked();
    if let Some(open) = state.open.iter_mut().find(|open| open.id == question.id) {
        open.stage = Stage::Person {
            lead: None,
            passed: false,
        };
        core.harness().questions.changed(&mut state);
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
        let mut raise = Some(raise_of(params, &text, &mut choices)?);
        (core.harness().questions).keep_pictures(&request.project, &mut choices)?;
        (core.harness().questions).keep_evidence(&request.project, &mut raise)?;
        let raise = raise.ok_or("the decision could not be raised")?;
        if let Some(foreign) = foreign_worker(core, &agent_id, &worker)? {
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
    let mut raise = raised
        .then(|| raise_of(params, &text, &mut choices))
        .transpose()?;
    let production = (raise.as_ref()).is_some_and(|raise| raise.reason == RaisedFor::Irreversible);
    if production && !blocked {
        return Err("A change to production waits for the person's answer; ask it with blocked: true, never going on with an assumption".into());
    }
    if let Some(raise) = raise.as_mut() {
        let (reading, asker) = (core.clone(), agent_id.clone());
        raise.asked_for = super::connection::blocking(move || Ok(asked_for(&reading, &asker)))
            .await
            .map_err(|error| error.to_string())?;
    }
    mark_pick(params, &mut choices)?;
    (core.harness().questions).keep_pictures(&request.project, &mut choices)?;
    (core.harness().questions).keep_evidence(&request.project, &mut raise)?;
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
            proof: None,
            dialog: None,
            raise,
        }],
    )?
    .remove(0);
    hand_to_lead(core, &question).await;
    let question = core
        .harness()
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
            proof: None,
            dialog: None,
            raise: None,
        }],
    )
    .map(drop)
}

/// Asks the person, never a lead, whether a slice's proof goes into its pull request, on
/// behalf of the agent that owns the slice. `text` is what would be written.
pub(crate) fn put_proof_to_person(
    core: &Core,
    agent_id: &str,
    proof: ProofAsk,
    header: String,
    text: String,
) -> Result<(), String> {
    let choice = |label: &str, detail: &str| QuestionChoice {
        label: label.to_owned(),
        detail: Some(detail.to_owned()),
        picture: None,
        recommended: false,
    };
    let project = proof.project.clone();
    open(
        core,
        agent_id,
        &project,
        vec![Asked {
            text: clipped(&text, MAX_QUESTION_BYTES),
            header: Some(clipped(&header, MAX_HEADER_CHARS)),
            choices: vec![
                choice(
                    "Post",
                    "Write it now, and keep it up to date on later pushes",
                ),
                choice("Skip", "Never write to this pull request"),
            ],
            multi: false,
            blocked: false,
            assumption: Some(
                "Nothing is written to the pull request until you post it.".to_owned(),
            ),
            secret: false,
            approval: None,
            proof: Some(proof),
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
        .chats()
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
            proof: None,
            dialog: Some(dialog),
            raise: None,
        }],
    )
    .map(drop)
}

/// Closes the question on `agent_id`'s dialog `only`, or on all its dialogs,
/// when they no longer wait.
pub(crate) fn withdraw_dialogs(core: &Core, agent_id: &str, only: Option<&str>) {
    let questions = &core.harness().questions;
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

/// The worker of `lead` on a linked host that `worker` names, by its id, its
/// name, or its name and host.
fn foreign_worker(
    core: &Core,
    lead: &str,
    worker: &str,
) -> Result<Option<super::hosts::team::Foreign>, String> {
    let (wanted, on) = super::messages::on_host(worker.trim().trim_start_matches('@'));
    let mut found: Vec<super::hosts::team::Foreign> = (core.hosts().team.all().into_iter())
        .filter(|foreign| foreign.parent.as_deref() == Some(lead))
        .filter(|foreign| {
            foreign.agent_id == wanted
                || foreign.name.eq_ignore_ascii_case(wanted)
                    && on.is_none_or(|on| {
                        super::hosts::name_of(core, &foreign.host).eq_ignore_ascii_case(on)
                    })
        })
        .collect();
    if found.len() > 1 {
        return Err(format!(
            "More than one of your agents on linked hosts is called {wanted}: {}. Set for to the agent id instead",
            found
                .iter()
                .map(|foreign| format!("{} on {}", foreign.agent_id, super::hosts::name_of(core, &foreign.host)))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    Ok(found.pop())
}

/// Whether question `id` asks for a change to production, which only its call answers.
pub(crate) fn asks_production(core: &Core, id: &str) -> bool {
    (core.harness().questions.state.locked().open.iter())
        .any(|question| question.id == id && question.production_call())
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
    let raise = Raise {
        asked_for: None,
        ..raise
    };
    let text = text_param(&json!({ "text": text }), "text", MAX_QUESTION_BYTES)?
        .ok_or("the question must be nonempty text")?;
    let mut choices = self::choices(Some(choices))?;
    for choice in &mut choices {
        choice.picture = None;
    }
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
        .names()
        .name(lead_id)
        .unwrap_or_else(|| lead_id.to_owned());
    let mut state = core.harness().questions.state.locked();
    let from = |question: &Question| {
        question.agent_id == wanted || question.name.eq_ignore_ascii_case(wanted)
    };
    let mut askers: Vec<(&str, &str)> = (state.open.iter())
        .filter(|question| from(question) && question.lead_id() == Some(lead_id))
        .map(|question| (question.agent_id.as_str(), question.project.as_str()))
        .collect();
    askers.sort_unstable();
    askers.dedup();
    if askers.len() > 1 {
        return Err(format!(
            "More than one of your agents called {wanted} has a question waiting: {}. Set for to the agent id instead",
            askers
                .iter()
                .map(|(id, project)| format!("{id} in {project}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
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
    if raise.reason == RaisedFor::Irreversible && !question.blocked {
        return Err(format!(
            "{wanted} goes on with an assumption while it waits, and a change to production waits for the person's answer. Answer {wanted}'s question as you tell it to ask again with blocked: true and wait (agent_send with answers), so it does not stand as decided, then raise the new one"
        ));
    }
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
    core.harness().questions.changed(&mut state);
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
                proof: None,
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
        .chats()
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

/// The chat's turn ended by itself: an offer of more work in its reply, which a yes or a no
/// answers, sends the agent back when a project rule covers it, and otherwise only shows as how
/// the turn left things. An agent with a lead, here or on a linked host, or that Auto runs,
/// answers to them instead, and one with a message still on its way is not done.
pub(crate) fn offers_in_reply(core: &Arc<Core>, agent_id: &str) {
    let Some(chat) = core.chats().get(agent_id) else {
        return;
    };
    let named = core.names().get(agent_id);
    if named.as_ref().is_some_and(|named| named.parent.is_some())
        || super::board::run_by_auto(core, agent_id)
        || core.harness().mailbox.has_waiting(agent_id)
        || chat.feed.has_queued()
    {
        return;
    }
    let Some(reply) = chat.feed.last_answer() else {
        return;
    };
    let offers = (asked_in(&reply).into_iter())
        .filter(|ask| ask.polar)
        .map(|offer| offer.text);
    let project = (named.map(|named| named.project))
        .unwrap_or_else(|| core.workspaces().project_of(&chat.launch.cwd));
    follow_rule(core, agent_id, &project, offers);
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
        let followed = &mut core.harness().questions.state.locked().followed_rules;
        followed.remove(agent_id);
        return;
    };
    let mut state = core.harness().questions.state.locked();
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
    let (open, answered) = {
        let state = core.harness().questions.state.locked();
        (state.open.clone(), state.answered.clone())
    };
    views_of(core, open, answered)
}

/// Each question's view, with the ticket of the card its asker holds.
fn views_of(core: &Core, open: Vec<Question>, answered: Vec<Answered>) -> Vec<AgentQuestion> {
    let open = (open.iter())
        .filter(|question| !(question.relayed.is_some() && question.relay_taken))
        .map(Question::view);
    let answered = answered.iter().map(|answered| {
        let mut view = answered.question.view();
        view.state = QuestionState::Answered;
        view.answer = answered.answer.clone();
        view
    });
    open.chain(answered)
        .map(|mut view| {
            view.ticket = super::board::ticket_of_agent(core, &view.agent_id);
            view
        })
        .collect()
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

pub(crate) trait AnswerParamsExt: Sized {
    /// Writes the files and context into `answer`: what was typed, the files, then the context,
    /// so the agent reads the same text whichever client answered. The files stay, for a chat to
    /// take as it takes the composer's.
    fn written_out(&mut self);
}

impl AnswerParamsExt for AnswerParams {
    fn written_out(&mut self) {
        let context = std::mem::take(&mut self.context);
        if self.paths.is_empty() && context.is_empty() {
            return;
        }
        let attached = (!self.paths.is_empty()).then(|| attached(&self.paths));
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

/// The files given with an answer as its text lists them.
pub(crate) fn attached(paths: &[String]) -> String {
    let listed: Vec<String> = paths.iter().map(|path| format!("- {path}")).collect();
    format!("Attached:\n{}", listed.join("\n"))
}

/// An answer's text without its list of `paths`, for an agent that takes the files themselves.
pub(crate) fn without_attached(text: &str, paths: &[String]) -> String {
    let list = attached(paths);
    let text = text.replacen(&format!("\n\n{list}"), "", 1);
    text.replacen(&list, "", 1).trim().to_owned()
}

/// The person's answers to questions asked together. Each asker gets all of
/// its answers in one message, so the first does not start a turn before the
/// rest arrive.
pub(crate) async fn answer_all(core: &Arc<Core>, mut params: AnswersParams) -> Result<(), String> {
    attach_picked_pictures(core, &mut params.answers);
    params
        .answers
        .iter_mut()
        .for_each(AnswerParams::written_out);
    let taken = {
        let mut state = core.harness().questions.state.locked();
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
        for (question, given, text) in &taken {
            if let (None, None, Some(text)) = (&question.from_host, &question.dialog, text) {
                let picked = given.choice.is_some() || given.choices.is_some();
                let text = match picked {
                    true => without_attached(text, &given.paths),
                    false => text.clone(),
                };
                state.answered.push(Answered {
                    question: question.clone(),
                    answer: (!question.hides_answer()).then_some(text),
                });
            }
        }
        let over = state.answered.len().saturating_sub(MAX_ANSWERED);
        state.answered.drain(..over);
        core.harness().questions.changed(&mut state);
        taken
    };
    let reopen = |questions: Vec<Question>| {
        let mut state = core.harness().questions.state.locked();
        state.answered.retain(|answered| {
            !questions
                .iter()
                .any(|question| question.id == answered.question.id)
        });
        state.open.extend(questions);
        core.harness().questions.changed(&mut state);
    };
    // Each answer as the asker gets it, with what was done about it when that is not the answer itself.
    let mut letters: Vec<(Question, String, Option<String>, Vec<String>)> = Vec::new();
    for (question, given, text) in taken {
        if let Some(host) = &question.from_host {
            let params = serde_json::to_value(&given).map_err(|error| error.to_string())?;
            let asked = serde_json::to_value(&question).map_err(|error| error.to_string())?;
            super::hosts::route::relay_answer(core, host, params, asked);
            continue;
        }
        let Some(text) = text else { continue };
        if let Some(proof) = &question.proof {
            super::board::proof::answered(core, proof, given.choice == Some(0));
            continue;
        }
        if let Some(dialog) = &question.dialog {
            if let Some(chat) = core.chats().get(&question.agent_id) {
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
            (None, true) => {
                tokio::task::spawn_blocking(move || sikemux_keychain::reference::resolve(&text))
                    .await
                    .map_err(|error| error.to_string())
                    .flatten()
                    .and_then(|secret| core.harness().questions.keep_secret(&question, &secret))
                    .map(|path| (secret_note(&path), None))
            }
            (None, false) => Ok((text, None)),
        };
        match told {
            Ok((text, done)) => letters.push((question, text, done, given.paths)),
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
        let own: Vec<&(Question, String, Option<String>, Vec<String>)> = letters
            .iter()
            .filter(|(question, ..)| question.agent_id == asker)
            .collect();
        let sent = own
            .iter()
            .map(|(question, text, done, paths)| {
                let note = match (done, &question.assumption) {
                    (Some(done), _) => done.clone(),
                    (None, Some(assumption)) => format!(
                        "The person answered the question you went on without. If that is not what you assumed ({assumption}), change course."
                    ),
                    (None, None) => "The person answered the question you asked; go on with it.".to_owned(),
                };
                (
                    answered(question, &question.asked),
                    text.clone(),
                    note,
                    paths.clone(),
                )
            })
            .collect();
        match messages::answer(core, asker.to_owned(), sent).await {
            Ok(delivered) if delivered["delivery"] == "waiting" => {}
            Ok(_) => core.harness().questions.delivered(asker),
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
        for (question, text, ..) in own {
            copy_to_lead(core, question, text).await;
        }
    }
    match failed {
        Some(error) => Err(error),
        None => Ok(()),
    }
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
            vec![(
                answered(question, &question.text),
                text.to_owned(),
                note,
                Vec::new(),
            )],
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
        return Ok(Some(with_picked(labels.join(", "), params)));
    }
    if let Some(choice) = params.choice {
        return question
            .choices
            .get(choice)
            .map(|choice| Some(with_picked(choice.label.clone(), params)))
            .ok_or_else(|| "That choice is not one of the question's".into());
    }
    reply_text(params).map(Some)
}

/// A picked choice's label and, when its question showed pictures, the pictures picked.
fn with_picked(label: String, params: &AnswerParams) -> String {
    let note = (params.answer.as_deref())
        .map(|note| without_attached(note, &params.paths))
        .filter(|note| !note.is_empty());
    let parts = [
        Some(label),
        note,
        (!params.paths.is_empty()).then(|| attached(&params.paths)),
    ];
    parts.into_iter().flatten().collect::<Vec<_>>().join("\n\n")
}

/// Lists the pictures of the choices picked among the answer's files, copied to where the
/// chat's files are kept, so the answer shows what was picked once the question is gone.
fn attach_picked_pictures(core: &Arc<Core>, answers: &mut [AnswerParams]) {
    let Some(dir) = core.config().attachment_dir.as_deref() else {
        return;
    };
    let shown: Vec<(usize, String)> = {
        let state = core.harness().questions.state.locked();
        (answers.iter().enumerate())
            .filter_map(|(at, given)| {
                let question = state.open.iter().find(|open| open.id == given.id)?;
                let picked = given
                    .choice
                    .into_iter()
                    .chain(given.choices.iter().flatten().copied());
                Some(
                    picked
                        .filter_map(|ix| question.choices.get(ix)?.picture.clone())
                        .map(move |picture| (at, picture))
                        .collect::<Vec<_>>(),
                )
            })
            .flatten()
            .collect()
    };
    for (at, picture) in shown {
        let from = Path::new(&picture);
        let Some(name) = from.file_name() else {
            continue;
        };
        let to = dir.join(name);
        if std::fs::create_dir_all(dir)
            .and_then(|()| std::fs::copy(from, &to))
            .is_ok()
        {
            answers[at].paths.push(to.to_string_lossy().into_owned());
        }
    }
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

/// Moves on questions their lead left, drops those of agents that are gone,
/// and tells the window and phones what changed. Runs when questions or the
/// agents change, and again when a lead's time to answer runs out.
pub(crate) fn settle(core: &Arc<Core>) {
    let (askers, leads): (HashSet<String>, HashSet<String>) = {
        let state = core.harness().questions.state.locked();
        if state.dirty {
            core.boards().wake.now();
        }
        if state.open.is_empty()
            && state.answered.is_empty()
            && state.told_phones.is_empty()
            && state.shown_devices.is_empty()
            && !state.dirty
        {
            drop(state);
            tell_calls(core);
            tell_window(core);
            return;
        }
        (
            state
                .open
                .iter()
                .chain(state.answered.iter().map(|answered| &answered.question))
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
            let state = (
                reachable(core, &lead),
                core.agent_now(&lead).maybe_mid_turn(),
            );
            (lead, state)
        })
        .collect();
    let unread: HashSet<String> = {
        let state = core.harness().questions.state.locked();
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
    let mut again = Vec::new();
    let mut left_over: Vec<String> = Vec::new();
    let left: Vec<Question>;
    let standing: Vec<Question>;
    {
        let mut state = core.harness().questions.state.locked();
        let before = state.open.len();
        state
            .open
            .retain(|question| !gone.contains(&question.agent_id));
        let answered = state.answered.len();
        state
            .answered
            .retain(|answered| !gone.contains(&answered.question.agent_id));
        let mut changed = state.open.len() != before || state.answered.len() != answered;
        for question in &mut state.open {
            let Stage::Lead {
                lead_id,
                lead_name,
                ended_at,
                handed_back,
                left_by_person,
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
            if *left_by_person && !busy && (!reachable || passed_over) {
                left_over.push(question.id.clone());
            } else if busy {
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
        let (gone_back, open) = std::mem::take(&mut state.open)
            .into_iter()
            .partition::<Vec<_>, _>(|question| left_over.contains(&question.id));
        left = gone_back;
        let (stands, open) = open.into_iter().partition::<Vec<_>, _>(Question::stands);
        state.open = open;
        changed |= !stands.is_empty() || !left.is_empty();
        standing = stands;
        if changed {
            core.harness().questions.changed(&mut state);
            core.boards().wake.now();
        }
    }
    for question in standing {
        stood(core, question);
    }
    for question in left {
        let core = core.clone();
        tokio::spawn(async move { left_to_asker(&core, &question).await });
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
    relay(core);
    if let Some(next) = next {
        core.harness().questions.wake.at(next);
    }
    tell_calls(core);
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
        let mut state = core.harness().questions.state.locked();
        let shown: HashMap<String, String> = state
            .open
            .iter()
            .filter(|question| question.listed_for_devices())
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
        let now: HashMap<String, Asking> = (state.open.iter())
            .filter(|question| {
                let waits = question.blocked || question.raise.is_some() || question.persons_call();
                waits && question.listed_for_devices()
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
        core.seen().active(&agent_id);
    }
    for (id, asking) in asked {
        core.note_activity(&asking.agent_id, Activity::NeedsYou);
        core.notify().send(Signal::Question {
            id,
            agent_id: asking.agent_id,
            name: asking.name,
            text: asking.text,
            at: asking.at,
        });
    }
    for (id, agent_id) in cleared {
        core.notify().send(Signal::QuestionCleared { id, agent_id });
    }
}

/// The questions put to the person, as calls. A call nobody is left to hear the answer of
/// closes.
fn tell_calls(core: &Core) {
    let wanted = {
        let state = core.harness().questions.state.locked();
        (state.open.iter())
            .filter(|question| question.listed_for_devices())
            .map(|question| (question.call(), question.allows()))
            .map(|(call, yes)| person_calls::Wanted {
                source: person_calls::Source::Question(call.id.clone()),
                call,
                key: None,
                yes,
            })
            .collect()
    };
    person_calls::sync_questions(core, wanted);
    person_calls::close_left(core, |agent_id| gone(core, agent_id));
}

/// What the asker hears when the person leaves its question to it.
const LEFT_TO_ASKER: &str =
    "The person left this to you. Go with what you would pick, and say in your answer what you chose.";

/// The person hands the question back: the lead it went to first decides it, and it never reaches
/// the person again. With no lead to take it, the asker goes with its own pick.
pub(crate) async fn hand_back(core: &Arc<Core>, id: &str) -> Result<(), String> {
    let lead = {
        let state = core.harness().questions.state.locked();
        let question = (state.open.iter()).find(|question| question.id == id);
        question.and_then(|question| match &question.stage {
            Stage::Person {
                lead: Some(lead), ..
            } => Some(lead.clone()),
            _ => None,
        })
    };
    let Some((lead_id, lead_name)) = lead.filter(|(lead_id, _)| reachable(core, lead_id)) else {
        return answer(
            core,
            AnswerParams {
                id: id.to_owned(),
                answer: Some(LEFT_TO_ASKER.to_owned()),
                ..AnswerParams::default()
            },
        )
        .await;
    };
    let question = {
        let mut state = core.harness().questions.state.locked();
        let question = (state.open.iter_mut())
            .find(|question| question.id == id)
            .ok_or("That question has been answered already")?;
        question.stage = Stage::Lead {
            lead_id: lead_id.clone(),
            lead_name,
            ended_at: None,
            handed_back: true,
            left_by_person: true,
        };
        let question = question.clone();
        core.harness().questions.changed(&mut state);
        question
    };
    let (text, _) = for_lead(&question);
    let name = &question.name;
    let note = format!(
        "The person left @{name}'s question to you. Decide it: answer with agent_send to {name}, setting answers to {id} and why. It does not go back to the person; if your turn ends without an answer, @{name} goes with its own pick."
    );
    let sent = messages::question(
        core,
        id.to_owned(),
        question.agent_id.clone(),
        lead_id,
        text,
        note,
    )
    .await;
    if sent.is_err() {
        let taken = {
            let mut state = core.harness().questions.state.locked();
            let at = state.open.iter().position(|open| open.id == id);
            let taken = at.map(|at| state.open.remove(at));
            core.harness().questions.changed(&mut state);
            taken
        };
        if let Some(question) = taken {
            left_to_asker(core, &question).await;
        }
    }
    Ok(())
}

/// Nobody decided a question the person handed back, so its asker goes with its own pick.
async fn left_to_asker(core: &Arc<Core>, question: &Question) {
    let note = "Nobody else decides this one: the person handed it back.".to_owned();
    let letter = (
        answered(question, &question.asked),
        LEFT_TO_ASKER.to_owned(),
        note,
        Vec::new(),
    );
    let _ = messages::answer(core, question.agent_id.clone(), vec![letter]).await;
}

/// Sends the window the open questions when they changed, or when a window
/// opened that has not had them.
fn tell_window(core: &Arc<Core>) {
    let window = core.window().client_id();
    let (open, answered) = {
        let mut state = core.harness().questions.state.locked();
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
        (state.open.clone(), state.answered.clone())
    };
    let views = views_of(core, open, answered);
    let request = json!({
        "id": "agent-questions",
        "project": "/",
        "agentId": null,
        "method": "agent.questions",
        "params": { "questions": views },
    });
    let Ok(call) = core.window().call(
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
            core.window().forget(id);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{Question, Questions};
    use sikemux_process::sync::Locked;

    #[test]
    fn a_question_another_host_took_still_hides_what_is_typed_to_its_asker_and_its_lead() {
        let taken: Question = serde_json::from_value(serde_json::json!({
            "id": "question-1",
            "agentId": "worker",
            "name": "Worker",
            "cli": "claude",
            "project": "widget",
            "asked": "Which token does staging take?",
            "text": "Which token does staging take?",
            "choices": [],
            "blocked": true,
            "assumption": null,
            "secret": false,
            "at": 1,
            "stage": { "stage": "person", "lead": ["lead", "Lead"], "passed": true },
            "raise": { "reason": "access", "breaks": "the deploy" },
            "relayed": "studio",
            "relayTaken": true,
        }))
        .expect("a question");
        assert!(!taken.listed_for_devices(), "it has no call on this host");
        let questions = Questions::new(None, |_| true);
        questions.state.locked().open.push(taken);
        assert!(questions.hears_hidden("worker"));
        assert!(questions.hears_hidden("lead"));
        assert!(!questions.hears_hidden("bystander"));
    }

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
}
