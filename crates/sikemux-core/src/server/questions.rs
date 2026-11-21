//! What `ask` answers, and the CLIs' own question tools with it. An agent's
//! question goes first to the agent that started it, which answers it or asks
//! the person itself; a lead that ends its turn without answering passes it on
//! to the person. An agent with no lead asks the person directly. The person
//! answers on the question's card, and the first answer reaches the asker as a
//! message.

pub(crate) use super::person_calls::{attached, without_attached};
use crate::server::activity::ActivityCore;
use crate::server::agent_now::AgentNowCore;
use crate::server::board_state::BoardParts;
use crate::server::changed::ChangedCore;
use crate::server::changed::What;
use crate::server::chat_state::ChatParts;
use crate::server::parts::Parts;
use crate::server::remote_state::RemoteParts;
use crate::server::serving::ServeParts;
use crate::server::workspace_state::WorkspaceParts;
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
use crate::sessions::asked::may_hold_secret;
use crate::text::clipped;
use sikemux_view::calls::{
    CallAsker, CallKind, CallLink, CallOpened, CallOption, CallPage, CallState, PersonCall,
    DO_DONE, DO_NOT_DOING, LOOK_SEEN,
};
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
/// A shorter answer is hidden only where it was answered, since it would match ordinary words.
const MIN_HIDDEN_SECRET_CHARS: usize = 4;
const HIDDEN: &str = "••••••••";
const MAX_QUESTION_BYTES: usize = 8 * 1024;
const MAX_CHOICES: usize = 9;
const MAX_PAGES: usize = 6;
const MAX_LABEL_CHARS: usize = 200;
const MAX_DETAIL_CHARS: usize = 400;
/// How recent the page an agent showed must be for a call about its work to link it unasked.
const LAST_PAGE_MS: u64 = 60 * 60_000;
/// A question's title on the tab the person picks it by.
const MAX_HEADER_CHARS: usize = 40;
const MAX_ANSWER_BYTES: usize = 64 * 1024;
const MAX_OPEN_PER_AGENT: usize = 8;
/// Answers kept to show while their askers have not read them; the oldest go first.
const MAX_ANSWERED: usize = 32;
/// A lead whose turn ended may still be handed the question back unread, so
/// it counts as passing it over only once this long has gone by.
const LEAD_GRACE_MS: u64 = 3_000;
const WINDOW_ANSWER_WAIT: Duration = Duration::from_secs(10);

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
    /// Put to the person. `lead` is the agent it went to first, which left it unanswered.
    #[serde(rename_all = "camelCase")]
    Person { lead: Option<(String, String)> },
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
    /// The asker waits for the answer; otherwise it goes on and hears the answer later.
    blocked: bool,
    /// The answer is a secret, such as a password, so the question goes
    /// straight to the person and the answer never enters a transcript.
    #[serde(default)]
    secret: bool,
    /// What the asker says makes it the person's to decide.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kind: Option<CallKind>,
    /// The agent whose work it is about, when that is not the asker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    about: Option<CallAsker>,
    /// The page that shows what it is about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    page: Option<CallPage>,
    /// Pages after the first, when it names several.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    more_pages: Vec<CallPage>,
    /// Where a do call's task is done, or the change a look call shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    link: Option<CallLink>,
    /// What the asker, or the agent it is about, opened for the person just before.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    opened: Vec<CallOpened>,
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
    /// The dialog in the asker's process that waits on the answer, which then
    /// goes to it rather than arriving as a message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dialog: Option<String>,
    /// The questions the agent it is about asked its asker, which wait for this one's answer
    /// rather than going to the person as well.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    holds: Vec<String>,
}

impl Question {
    fn view(&self) -> AgentQuestion {
        let (state, via_note) = match &self.stage {
            Stage::Lead { .. } => (QuestionState::Lead, None),
            Stage::Person { lead } => (
                QuestionState::Open,
                lead.as_ref()
                    .map(|(_, name)| format!("@{name} didn't answer")),
            ),
        };
        AgentQuestion {
            id: self.id.clone(),
            agent_id: self.agent_id.clone(),
            name: self.name.clone(),
            cli: self.cli.clone(),
            project: self.project.clone(),
            asked: self.asked.clone(),
            text: self.text.clone(),
            header: self.header.clone(),
            choices: self.choices.clone(),
            multi: false,
            blocked: self.blocked,
            assumption: None,
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

    /// Every page it names, the first one first.
    fn pages(&self) -> impl Iterator<Item = &CallPage> {
        self.page.iter().chain(&self.more_pages)
    }

    /// Its call to the person is one for a change to production.
    fn production_call(&self) -> bool {
        !self.secret && self.approval.is_some()
    }

    /// The option of its call that lets a plugin call it holds run.
    fn allows(&self) -> Option<String> {
        let approving = self.approval.as_ref()?.approving()?;
        Some((approving + 1).to_string())
    }

    /// It as a call to the person.
    fn call(&self) -> PersonCall {
        let kind = match () {
            _ if self.secret => CallKind::Access,
            _ if self.approval.is_some() => CallKind::Production,
            _ => self.kind.unwrap_or(CallKind::Taste),
        };
        let (options, pick) = match kind {
            CallKind::Do => (form_options(&[DO_DONE, DO_NOT_DOING], &self.choices), None),
            CallKind::Look => (form_options(&[LOOK_SEEN], &self.choices), None),
            _ => call_options(&self.choices),
        };
        PersonCall {
            id: self.id.clone(),
            project: self.project.clone(),
            kind,
            title: self.header.clone(),
            asker: CallAsker {
                agent_id: self.agent_id.clone(),
                name: self.name.clone(),
                cli: self.cli.clone(),
                chief: false,
            },
            joined: Vec::new(),
            about: self.about.clone(),
            page: self.page.clone(),
            more_pages: self.more_pages.clone(),
            shown_in: Vec::new(),
            link: self.link.clone(),
            question: self.text.clone(),
            context: None,
            assumption: None,
            options,
            pick,
            against: None,
            evidence: Vec::new(),
            opened: self.opened.clone(),
            action: None,
            brief: None,
            place: None,
            can_hand_back: false,
            multi: false,
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
        matches!(self.stage, Stage::Person { .. })
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
            } => lead_id == lead,
            Stage::Person { lead: None } => false,
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
    /// The agents whose last turn ended saying they wait with nothing set to wake them: false when
    /// that turn sent them back, true when they ended so again after being sent back.
    unwoken: HashMap<String, bool>,
    /// Oldest first.
    answered: Vec<Answered>,
    /// Something changed that the window has not been sent yet.
    dirty: bool,
    /// The window connection last sent the questions.
    told_window: Option<ClientId>,
    /// The person's open questions phones were told of, and whose they are.
    told_phones: HashMap<String, String>,
    /// Every question put to the person that devices'
    /// views last showed, and whose they are.
    shown_devices: HashMap<String, String>,
    keeper: Keeper,
    /// The open questions also put to the person on another host, and which.
    relayed_to: HashMap<String, String>,
    /// Notes for askers whose pick was cleared, by asker, not yet sent.
    unpicked: Vec<(String, String)>,
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
    /// It asks for a secret, such as a password or a token.
    pub secret: bool,
    /// The answers it can be given in one tap, in order.
    pub answers: Vec<String>,
    /// The picture each of `answers` shows, when its asker gave them.
    pub pictures: Vec<Option<String>>,
    pub at: u64,
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

/// The fixed answers of a do or look call, by the ids clients know them by, over the choices the
/// question holds in the same order.
fn form_options(ids: &[&str], choices: &[QuestionChoice]) -> Vec<CallOption> {
    (ids.iter().zip(choices))
        .map(|(id, choice)| CallOption {
            id: (*id).to_owned(),
            label: choice.label.clone(),
            detail: None,
            picture: None,
        })
        .collect()
}

/// The choices a do or look call is answered by, in the order of [`form_options`].
fn form_choices(kind: CallKind) -> Option<Vec<QuestionChoice>> {
    let labels: &[&str] = match kind {
        CallKind::Do => &["Done", "Not doing"],
        CallKind::Look => &["Seen"],
        _ => return None,
    };
    Some(
        (labels.iter())
            .map(|label| QuestionChoice {
                label: (*label).to_owned(),
                detail: None,
                picture: None,
                recommended: false,
            })
            .collect(),
    )
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
    blocked: bool,
    secret: bool,
    kind: Option<CallKind>,
    about: Option<CallAsker>,
    pages: Vec<CallPage>,
    link: Option<CallLink>,
    approval: Option<Approval>,
    dialog: Option<String>,
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
            let shown: HashSet<String> = chosen.collect();
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

    /// The text with every secret the person answered an agent with drawn as dots, so an agent
    /// passing one on never puts it in front of another agent or the person.
    pub(crate) fn hide_secrets(&self, text: &str) -> String {
        let Some(root) = self.secrets.as_deref() else {
            return text.to_owned();
        };
        let mut kept: Vec<String> = (std::fs::read_dir(root).into_iter().flatten().flatten())
            .flat_map(|agent| {
                std::fs::read_dir(agent.path())
                    .into_iter()
                    .flatten()
                    .flatten()
            })
            .filter_map(|file| std::fs::read_to_string(file.path()).ok())
            .map(|secret| secret.trim().to_owned())
            .filter(|secret| secret.chars().count() >= MIN_HIDDEN_SECRET_CHARS)
            .collect();
        kept.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
        kept.iter().fold(text.to_owned(), |text, secret| {
            text.replace(secret.as_str(), HIDDEN)
        })
    }

    /// Copies the pictures of the raise's evidence, as [`Self::keep_pictures`] copies choices'.
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

    /// Each agent that stopped for the answer to a question its lead has, with that lead's name
    /// and when it asked; its oldest such question when it has several.
    pub(crate) fn waiting_on_leads(&self) -> HashMap<String, super::turn_state::OnAgent> {
        let mut waiting = HashMap::new();
        for question in self.state.locked().open.iter().filter(|q| q.blocked) {
            if let Stage::Lead {
                lead_id, lead_name, ..
            } = &question.stage
            {
                waiting.entry(question.agent_id.clone()).or_insert_with(|| {
                    super::turn_state::OnAgent {
                        id: lead_id.clone(),
                        name: lead_name.clone(),
                        since: question.at,
                    }
                });
            }
        }
        waiting
    }

    /// The questions put to the person, oldest first: of those an agent waits on, only its
    /// latest.
    pub(crate) fn for_person(&self) -> Vec<ForPerson> {
        let state = self.state.locked();
        let questions: Vec<ForPerson> = (state.open.iter())
            .filter(|question| question.listed_for_devices())
            .map(|question| ForPerson {
                id: question.id.clone(),
                agent_id: question.agent_id.clone(),
                text: question.text.clone(),
                blocked: question.blocked,
                secret: question.secret,
                answers: (question.choices.iter())
                    .map(|choice| choice.label.clone())
                    .collect(),
                pictures: match question
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
                pick: recommended_of(&question.choices),
            })
            .collect();
        let mut latest: HashMap<&str, (u64, &str)> = HashMap::new();
        for question in questions.iter().filter(|question| question.blocked) {
            let kept = latest.entry(&question.agent_id).or_insert((0, ""));
            if kept.1.is_empty() || kept.0 < question.at {
                *kept = (question.at, &question.id);
            }
        }
        let shown: HashSet<String> = latest.values().map(|(_, id)| (*id).to_owned()).collect();
        (questions.iter())
            .filter(|question| !question.blocked || shown.contains(&question.id))
            .cloned()
            .collect()
    }

    /// The agent began another turn, as when the person wrote to it. A question it stopped for
    /// no longer waits, unless the call it raised is still `open`: a call waits until the person
    /// answers it. A question it did not stop for, and a plugin call waiting for the person's
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
                question.id == id && question.agent_id == agent_id && question.went_to(lead)
            })
            .map(|question| question.asked.clone())
    }

    /// Why `lead` can't answer `id` for the agent `name`: the call is the lead's own, or no
    /// such question is open.
    pub(crate) fn not_asked(&self, lead: &str, name: &str, id: &str) -> String {
        let state = self.state.locked();
        let own =
            (state.open.iter()).find(|question| question.id == id && question.agent_id == lead);
        match own {
            Some(call) => format!(
                "{id} is your own open call \"{}\", not a question {name} asked you. If it is settled, withdraw it with ask, edit: \"{id}\" and withdraw: true; then send this without answers",
                call.header.as_deref().unwrap_or(&call.text)
            ),
            None => format!("{name} has no open question {id} that it asked you; it was answered already, or leave out answers to send this as a plain message"),
        }
    }

    /// `agent`'s open calls that `keep` lets through, as `id "title"` items.
    fn calls_of(&self, agent: &str, keep: impl Fn(&Question) -> bool) -> Vec<String> {
        let state = self.state.locked();
        (state.open.iter())
            .filter(|question| question.agent_id == agent && keep(question))
            .map(|call| {
                format!(
                    "{} \"{}\"",
                    call.id,
                    call.header.as_deref().unwrap_or(&call.text)
                )
            })
            .collect()
    }

    /// What `agent` is told with the person's message while calls of its own wait on them.
    pub(crate) fn open_calls_note(&self, agent: &str) -> Option<String> {
        let calls = self.calls_of(agent, |call| matches!(call.stage, Stage::Person { .. }));
        (!calls.is_empty()).then(|| {
            format!(
                "Your calls still open to the person: {}. If this message settles one, withdraw it with ask, edit: its id and withdraw: true.",
                calls.join("; ")
            )
        })
    }

    /// What `lead` is told with a report from `worker` while calls of the lead's about it are open.
    pub(crate) fn calls_about_note(&self, lead: &str, worker: &str) -> Option<String> {
        let calls = self.calls_of(lead, |call| {
            call.about
                .as_ref()
                .is_some_and(|about| about.agent_id == worker)
        });
        (!calls.is_empty()).then(|| {
            format!(
                "Your open calls about this agent: {}. If this report settles one, withdraw it with ask, edit: its id and withdraw: true.",
                calls.join("; ")
            )
        })
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
    /// The person wrote to `agent_id` directly, so a call about its work no longer stands behind
    /// the pick its asker made before: the pick is cleared and the asker told to edit or withdraw.
    pub(crate) fn steered(&self, agent_id: &str) {
        let mut state = self.state.locked();
        let mut notes = Vec::new();
        for question in &mut state.open {
            let about = question
                .about
                .as_ref()
                .filter(|about| about.agent_id == agent_id);
            let Some(about) = about.filter(|_| question.asks_person()) else {
                continue;
            };
            let picked = question
                .choices
                .iter_mut()
                .filter(|choice| choice.recommended);
            if picked.map(|choice| choice.recommended = false).count() == 0 {
                continue;
            }
            let named = question
                .header
                .clone()
                .unwrap_or_else(|| clipped(&question.text, 80));
            let note = format!(
                "The person wrote to @{} directly after your call \"{named}\", so its pick was \
                 cleared. If it no longer stands, edit it with ask edit: \"{}\" or withdraw it \
                 with withdraw: true.",
                about.name, question.id
            );
            notes.push((question.agent_id.clone(), note));
        }
        if !notes.is_empty() {
            state.unpicked.extend(notes);
            self.changed(&mut state);
        }
    }

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
/// the record, with who decided and why. An answer the question hides goes on it only as answered
/// privately, without the lead's why, which may repeat it.
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
    let (asked, text, why) = match question.secret {
        true => (
            private_title(&question),
            format!(
                "@{} answered @{}'s question privately",
                core.names().name_or_id(lead),
                question.name
            ),
            None,
        ),
        false => (Some(question.asked.clone()), answer.to_owned(), why),
    };
    super::board::goal::keep_settled(
        core,
        super::board::goal::Settled {
            project: question.project,
            by: lead.to_owned(),
            holder: question.agent_id,
            asked,
            text,
            why,
        },
    );
}

/// What the record names a question whose answer is hidden by: its words, or else its header,
/// whichever may not hold a secret itself.
fn private_title(question: &Question) -> Option<String> {
    (std::iter::once(&question.asked).chain(&question.header))
        .find(|title| !may_hold_secret(title))
        .cloned()
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

/// Marks the choice labelled `pick` as the one the asker recommends, when there is one.
fn mark_pick(pick: &str, choices: &mut [QuestionChoice]) {
    let pick = pick.trim();
    if let Some(at) = (choices.iter()).position(|choice| choice.label.eq_ignore_ascii_case(pick)) {
        for (index, choice) in choices.iter_mut().enumerate() {
            choice.recommended = index == at;
        }
    }
}

/// A choice's label without the "(recommended)" the agent put after it, and whether it did.
fn recommended(label: &str) -> (&str, bool) {
    const MARK: &str = "(recommended)";
    let label = label.trim();
    let cut = (label.len().checked_sub(MARK.len())).and_then(|at| label.split_at_checked(at));
    match cut {
        Some((kept, mark)) if mark.eq_ignore_ascii_case(MARK) => (kept.trim_end(), true),
        _ => (label, false),
    }
}

/// Whether the person closed the agent, so its questions go with it.
fn gone(core: &Core, agent_id: &str) -> bool {
    !core.agent_now(agent_id).running() && core.workspaces().closed(agent_id)
}

/// Whether a message reaches the agent now: it runs, or it sleeps in an open
/// window that wakes it. The window may not have said yet which chats it holds.
fn reachable(core: &Core, agent_id: &str) -> bool {
    let host_away = (core.hosts().team.get(agent_id))
        .is_some_and(|foreign| core.hosts().long_away(&foreign.host));
    if host_away {
        return false;
    }
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
    (core.names().get(agent_id)?.parent).filter(|lead| reachable(core, lead))
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
        },
        None => Stage::Person { lead: None },
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
                || asked.dialog.is_some()
                || asked.kind == Some(CallKind::Look)
            {
                Stage::Person { lead: None }
            } else {
                stage.clone()
            },
            secret: asked.secret,
            kind: asked.kind,
            opened: super::chief::opened_lately(
                core,
                asked
                    .about
                    .as_ref()
                    .map_or(agent_id, |about| &about.agent_id),
                LAST_PAGE_MS,
            ),
            about: asked.about,
            page: asked.pages.first().cloned(),
            more_pages: asked.pages.iter().skip(1).cloned().collect(),
            link: asked.link,
            id: format!("question-{}", uuid::Uuid::new_v4().simple()),
            agent_id: agent_id.to_owned(),
            name: name.clone(),
            cli: cli.clone(),
            project: project.clone(),
            asked: asked.text.clone(),
            text: asked.text,
            header: asked.header,
            choices: asked.choices,
            blocked: asked.blocked,
            at,
            relayed: None,
            relay_taken: false,
            undelivered: None,
            from_host: None,
            approval: asked.approval,
            dialog: asked.dialog,
            holds: Vec::new(),
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

fn moves_on(question: &Question) -> &'static str {
    match question.blocked {
        true => {
            "Stop here and end your turn without guessing: the answer comes to you as a message."
        }
        false => "Carry on with your work: the answer comes to you as a message.",
    }
}

/// What the asker is told about where its question went.
fn sent_note(question: &Question) -> String {
    let to = match &question.stage {
        Stage::Lead { lead_name, .. } => format!("Sent to @{lead_name}, your lead."),
        Stage::Person { .. } => "Sent to the person.".to_owned(),
    };
    format!("{to} {}", moves_on(question))
}

/// The question as the lead reads it.
fn for_lead(question: &Question) -> (String, String) {
    let name = &question.name;
    let mut text = match (question.kind, &question.header) {
        (Some(CallKind::Do), Some(title)) => {
            format!("A task for the person, {title}: {}", question.text)
        }
        _ => question.text.clone(),
    };
    if let Some(link) = &question.link {
        text.push_str(&format!("\n\n{}: {}", link.label, link.url));
    }
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
    if question.blocked {
        text.push_str(&format!(
            "\n\n@{name} waits for the answer before it goes on."
        ));
    }
    let id = &question.agent_id;
    let note = format!(
        "@{name} ({id}), an agent you started, asks you this. Answer with agent_send to {id}, setting answers to {}. If only the person can answer it, ask them yourself with ask, setting about to {}, and their answer reaches @{name} too. If your turn ends with neither, it goes to the person.",
        question.id, question.id
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
        open.stage = Stage::Person { lead: None };
        core.harness().questions.changed(&mut state);
    }
}

pub(crate) async fn ask(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    let agent_id = request
        .agent_id
        .clone()
        .ok_or("ask needs the calling agent's id")?;
    let params = &request.params;
    if let Some(id) = text_param(params, "edit", 64)? {
        return edit(core, request, &agent_id, &id).await;
    }
    let text = text_param(params, "question", MAX_QUESTION_BYTES)?
        .ok_or("question must be what you want to know, as nonempty text")?;
    let mut choices = choices(params.get("choices"))?;
    let header = (text_param(params, "title", MAX_QUESTION_BYTES)?)
        .or(text_param(params, "header", MAX_QUESTION_BYTES)?)
        .map(|header| clipped(&header, MAX_HEADER_CHARS));
    let flag = |key: &str| match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(on)) => Ok(Some(*on)),
        Some(_) => Err(format!("{key} must be a boolean")),
    };
    let kind = (params.get("kind")).and_then(|kind| CallKind::deserialize(kind).ok());
    let secret = flag("secret")?.unwrap_or(false);
    let look = kind == Some(CallKind::Look);
    let blocked = secret || (!look && flag("blocked")?.unwrap_or(true));
    let link = link_param(params.get("link"))?;
    let pages_wanted = pages_param(params)?;
    if let Some(form) = kind.and_then(form_choices) {
        formed(
            core,
            &agent_id,
            look,
            header.is_some(),
            (!pages_wanted.is_empty(), &link),
        )?;
        choices = form;
    } else if let Some(pick) = params.get("pick").and_then(Value::as_str) {
        mark_pick(pick, &mut choices);
    }
    (core.harness().questions).keep_pictures(&request.project, &mut choices)?;
    let about_wanted = text_param(params, "about", 200)?;
    let (about, held_question) = match &about_wanted {
        Some(wanted) => match handed_question(core, &agent_id, wanted) {
            Some(about) => (Some(about), Some(wanted.clone())),
            None => (about_of(core, &agent_id, wanted).await, None),
        },
        None => (None, None),
    };
    let (pages, unknown_pages): (Vec<_>, Vec<_>) = (pages_wanted.iter())
        .map(|id| page_of(core, id).ok_or(id.clone()))
        .partition(Result::is_ok);
    let mut pages: Vec<CallPage> = pages.into_iter().flatten().collect();
    if pages_wanted.is_empty() {
        pages.extend((about.as_ref()).and_then(|about| last_page_of(core, &about.agent_id)));
    }
    let unknown_pages: Vec<String> = unknown_pages.into_iter().filter_map(Result::err).collect();
    let question = open(
        core,
        &agent_id,
        &request.project,
        vec![Asked {
            text,
            header,
            choices,
            blocked,
            secret,
            kind,
            about,
            pages,
            link,
            approval: None,
            dialog: None,
        }],
    )?
    .remove(0);
    link_pages(core, &question).await;
    hold_for(core, &question, held_question.as_deref());
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
            Stage::Person { .. } => "the person".to_owned(),
        },
        "note": left_off(sent_note(&question), about_wanted.filter(|_| question.about.is_none()), &unknown_pages),
    }))
}

/// Changes or withdraws the open question `id` that `agent_id` asked, as the edit of an ask says;
/// the person's open call then shows the change, or closes as withdrawn.
async fn edit(
    core: &Arc<Core>,
    request: &HarnessRequest,
    agent_id: &str,
    id: &str,
) -> Result<Value, String> {
    let params = &request.params;
    let withdraw = matches!(params.get("withdraw"), Some(Value::Bool(true)));
    let text = text_param(params, "question", MAX_QUESTION_BYTES)?;
    let header = (text_param(params, "title", MAX_QUESTION_BYTES)?)
        .map(|header| clipped(&header, MAX_HEADER_CHARS));
    let mut choices = match params.get("choices") {
        None | Some(Value::Null) => None,
        given => Some(choices(given)?),
    };
    if let Some(choices) = &mut choices {
        (core.harness().questions).keep_pictures(&request.project, choices)?;
    }
    let pick = params.get("pick").and_then(Value::as_str);
    let link = link_param(params.get("link"))?;
    let pages = (pages_param(params)?.iter())
        .map(|id| page_of(core, id).ok_or(format!("no page {id} to show")))
        .collect::<Result<Vec<_>, _>>()?;
    let question = {
        let questions = &core.harness().questions;
        let mut state = questions.state.locked();
        let at = own_call(&state.open, agent_id, id)?;
        if withdraw {
            let question = state.open.remove(at);
            questions.changed(&mut state);
            drop(state);
            if let Some(lead) = question.lead_id() {
                messages::withdraw(core, lead, &question.id);
            }
            return Ok(json!({ "withdrawn": question.id }));
        }
        let question = state.open.get_mut(at).ok_or("that call just closed")?;
        if question.kind.and_then(form_choices).is_some() && (choices.is_some() || pick.is_some()) {
            return Err(
                "a do or look call's choices are fixed: edit its title, question, pages or link"
                    .into(),
            );
        }
        if let Some(text) = text {
            question.asked = text.clone();
            question.text = text;
        }
        if header.is_some() {
            question.header = header;
        }
        if let Some(choices) = choices {
            question.choices = choices;
        }
        if let Some(pick) = pick {
            mark_pick(pick, &mut question.choices);
        }
        if !pages.is_empty() {
            question.page = pages.first().cloned();
            question.more_pages = pages.into_iter().skip(1).collect();
        }
        if link.is_some() {
            question.link = link;
        }
        let question = question.clone();
        questions.changed(&mut state);
        question
    };
    link_pages(core, &question).await;
    Ok(json!({ "edited": question.id }))
}

/// Where in `open` the call of `agent_id` that `wanted` names is: by its id, or by its title
/// when exactly one of its open calls has that title.
fn own_call(open: &[Question], agent_id: &str, wanted: &str) -> Result<usize, String> {
    let mine = |open: &&Question| open.agent_id == agent_id;
    if let Some(at) = open
        .iter()
        .position(|call| mine(&call) && call.id == wanted)
    {
        return Ok(at);
    }
    let wanted = wanted.trim().trim_matches('"');
    let titled: Vec<usize> = (open.iter().enumerate())
        .filter(|(_, call)| mine(call))
        .filter(|(_, call)| {
            (call.header.as_deref()).is_some_and(|title| title.trim().eq_ignore_ascii_case(wanted))
        })
        .map(|(at, _)| at)
        .collect();
    match titled[..] {
        [at] => Ok(at),
        [] => {
            let yours: Vec<String> = (open.iter().filter(mine))
                .map(|call| format!("{} \"{}\"", call.id, call.header.as_deref().unwrap_or("")))
                .collect();
            Err(match yours.is_empty() {
                true => "you have no open call: it was answered or withdrawn".to_owned(),
                false => format!(
                    "no open call of yours has that id or title; yours: {}",
                    yours.join(", ")
                ),
            })
        }
        _ => Err("several open calls of yours have that title: edit one by its id".into()),
    }
}

/// Refuses a do or look call that lacks what it is drawn with, and a look from an agent that leads
/// no one: a look shows the person finished work, which only a lead knows is finished.
fn formed(
    core: &Core,
    agent_id: &str,
    look: bool,
    titled: bool,
    (paged, link): (bool, &Option<CallLink>),
) -> Result<(), String> {
    if !titled {
        return Err("a do or look call needs a title naming it in a few words".into());
    }
    if !look {
        return Ok(());
    }
    if !paged && link.is_none() {
        return Err("a look call needs the page or the link it shows".into());
    }
    let leads =
        !core.names().workers(agent_id).is_empty() || super::chief::all(core).contains(agent_id);
    match leads {
        true => Ok(()),
        false => Err("only a lead raises a look call: tell your lead the work is finished".into()),
    }
}

/// The `link` of an ask: where a do call's task is done, or what a look call shows.
fn link_param(value: Option<&Value>) -> Result<Option<CallLink>, String> {
    let link = match value {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::Object(link)) => link,
        Some(_) => return Err("link must be an object with a label and a url".into()),
    };
    let text = |key: &str| {
        (link.get(key).and_then(Value::as_str))
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };
    let url = text("url").ok_or("link needs a url")?;
    let label = text("label").unwrap_or_else(|| url.clone());
    Ok(Some(CallLink {
        label: clipped(&label, MAX_LABEL_CHARS),
        url: clipped(&url, MAX_QUESTION_BYTES),
    }))
}

/// Has `call` hold the question the agent it is about asked its asker, so a lead that asks the
/// person about its worker's question raises one call, whose answer goes to both. Unless the
/// asker named the question, only a lone one no other call holds is held: the answer fits no other.
fn hold_for(core: &Core, call: &Question, named: Option<&str>) {
    let Some(about) = call.about.as_ref().filter(|_| !call.secret) else {
        return;
    };
    let questions = &core.harness().questions;
    let mut state = questions.state.locked();
    let held_elsewhere =
        |id: &str| (state.open.iter()).any(|open| open.holds.iter().any(|held| held == id));
    let held: Vec<String> = (state.open.iter())
        .filter(|open| {
            open.agent_id == about.agent_id
                && open.lead_id() == Some(call.agent_id.as_str())
                && named.is_none_or(|named| open.id == named)
                && !held_elsewhere(&open.id)
        })
        .map(|open| open.id.clone())
        .collect();
    if held.len() != 1 {
        return;
    }
    if let Some(open) = state.open.iter_mut().find(|open| open.id == call.id) {
        open.holds = held;
        questions.changed(&mut state);
    }
}

/// The agent that asked `asker` the open question `id`, when it is one.
fn handed_question(core: &Core, asker: &str, id: &str) -> Option<CallAsker> {
    let question = (core.harness().questions.state.locked().open.iter())
        .find(|open| open.id == id && open.lead_id() == Some(asker))
        .cloned()?;
    Some(CallAsker {
        chief: super::chief::all(core).contains(&question.agent_id),
        agent_id: question.agent_id,
        name: question.name,
        cli: question.cli,
    })
}

/// The note on an asked question, saying which of its hints the card leaves off.
fn left_off(note: String, about: Option<String>, pages: &[String]) -> String {
    let about = about.map(|about| {
        format!(" The card names no other agent: {about} is you, or no open agent is called that.")
    });
    let pages = (!pages.is_empty()).then(|| {
        format!(
            " The card leaves off {}: no page you showed with page_show has that id.",
            pages.join(", ")
        )
    });
    format!(
        "{note}{}{}",
        about.unwrap_or_default(),
        pages.unwrap_or_default()
    )
}

/// The page ids an ask names, by `page` and then `pages`, each once.
fn pages_param(params: &Value) -> Result<Vec<String>, String> {
    let mut ids: Vec<String> = text_param(params, "page", 64)?.into_iter().collect();
    match params.get("pages") {
        None | Some(Value::Null) => {}
        Some(Value::Array(list)) if list.len() <= MAX_PAGES => {
            for id in list {
                let id = (id.as_str())
                    .ok_or("pages must be a list of page ids")?
                    .trim();
                if !id.is_empty() && !ids.iter().any(|kept| kept == id) {
                    ids.push(id.to_owned());
                }
            }
        }
        Some(Value::Array(_)) => return Err(format!("pages holds at most {MAX_PAGES}")),
        Some(_) => return Err("pages must be a list of page ids".into()),
    }
    Ok(ids)
}

/// Records each page `question` names as linked to its call, so the page knows its call.
async fn link_pages(core: &Core, question: &Question) {
    let Some(data_dir) = core.config().data_dir.clone() else {
        return;
    };
    let pages: Vec<String> = question.pages().map(|page| page.id.clone()).collect();
    let call = question.id.clone();
    let linked = move || {
        for page in &pages {
            super::harness::page_index::link_call(&data_dir, page, &call);
        }
        Ok(())
    };
    let _ = super::connection::blocking(linked).await;
}

/// The open agent `wanted` names, by name or id, looked for in the asker's project first. None
/// when it is the asker itself, which the card already names.
async fn about_of(core: &Arc<Core>, asker: &str, wanted: &str) -> Option<CallAsker> {
    let wanted = wanted.trim_start_matches('@').to_owned();
    let (core, asker) = (core.clone(), asker.to_owned());
    tokio::task::spawn_blocking(move || {
        let project = core.names().get(&asker).map(|named| named.project);
        let found = (super::roster::listed(&core).into_iter())
            .filter(|listed| {
                listed.agent_id == wanted
                    || (listed.name.as_deref())
                        .is_some_and(|name| name.eq_ignore_ascii_case(&wanted))
            })
            .min_by_key(|listed| Some(&listed.project) != project.as_ref())?;
        (found.agent_id != asker).then(|| CallAsker {
            chief: super::chief::all(&core).contains(&found.agent_id),
            name: found.name.unwrap_or(wanted),
            cli: found.cli,
            agent_id: found.agent_id,
        })
    })
    .await
    .ok()
    .flatten()
}

/// Page `id`, kept from an agent's page_show, by its title.
fn page_of(core: &Core, id: &str) -> Option<CallPage> {
    let dir = core.config().data_dir.as_ref()?.join(crate::pages::FOLDER);
    Some(CallPage {
        title: crate::pages::title(&dir, id)?,
        id: id.to_owned(),
    })
}

/// The page `agent` showed in the last hour, which a call about its work links when the asker
/// named none.
fn last_page_of(core: &Core, agent: &str) -> Option<CallPage> {
    let page = super::chief::last_page(core, agent, LAST_PAGE_MS)?;
    page_of(core, &page.id)
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
            blocked: true,
            secret: false,
            kind: None,
            about: None,
            pages: Vec::new(),
            link: None,
            approval: Some(approval),
            dialog: None,
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
            blocked: true,
            secret: false,
            kind: None,
            about: None,
            pages: Vec::new(),
            link: None,
            approval: None,
            dialog: Some(dialog),
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

/// Whether question `id` asks for a change to production, which only its call answers.
pub(crate) fn asks_production(core: &Core, id: &str) -> bool {
    (core.harness().questions.state.locked().open.iter())
        .any(|question| question.id == id && question.production_call())
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
                choices: choices(options.map(Value::Array).as_ref()).unwrap_or_default(),
                blocked: true,
                secret: question["isSecret"].as_bool().unwrap_or(false),
                kind: None,
                about: None,
                pages: Vec::new(),
                link: None,
                approval: None,
                dialog: None,
            })
        })
        .collect();
    if asked.is_empty() {
        return "Sikemux could not read that question; ask it again with the ask tool.".into();
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

/// The chat's turn ended by itself: what its reply offers, and what it says it waits on.
pub(crate) fn reply_read(core: &Arc<Core>, agent_id: &str) {
    offers_in_reply(core, agent_id);
    waits_in_reply(core, agent_id);
}

/// An offer of more work in a reply, which a yes or a no answers, sends the agent back when a
/// project rule covers it, and otherwise only shows as how the turn left things. An agent with a
/// lead, here or on a linked host, answers to them instead, and one with a message still on its
/// way is not done.
fn offers_in_reply(core: &Arc<Core>, agent_id: &str) {
    let Some(chat) = core.chats().get(agent_id) else {
        return;
    };
    let named = core.names().get(agent_id);
    if named.as_ref().is_some_and(|named| named.parent.is_some())
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

/// A worker whose reply says it waits, with nothing set to wake it, is sent back once to set a
/// wake or say what is left for its lead. Ending that way again makes its report urgent.
fn waits_in_reply(core: &Arc<Core>, agent_id: &str) {
    let caught = unwoken(core, agent_id);
    let mut state = core.harness().questions.state.locked();
    let Some(to) = caught else {
        state.unwoken.remove(agent_id);
        return;
    };
    if state.unwoken.insert(agent_id.to_owned(), true).is_some() {
        return;
    }
    state.unwoken.insert(agent_id.to_owned(), false);
    drop(state);
    let lead = (core.names().name(&to)).map_or("your lead".to_owned(), |name| format!("@{name}"));
    let text = format!(
        "Your turn ended saying you wait, but nothing is set to wake you, so you would sit until someone writes to you. If you wait on something, set trigger_add for it now: process, agent or ci when it is one, else every with a check that prints once it is ready, and a timeout. If it is not yours to wait on, say plainly what is left for {lead} and end your turn."
    );
    let (core, agent_id) = (core.clone(), agent_id.to_owned());
    tokio::spawn(async move {
        let why = "Sent by Sikemux itself because your turn ended waiting with nothing set to wake you, not by the person.";
        if let Err(error) = messages::send_back(&core, agent_id, text, why).await {
            eprintln!("sikemux core: an agent waiting on nothing was not sent back: {error}");
        }
    });
}

/// Whoever hears about a worker whose reply says it waits while nothing is set to wake it: its
/// lead while open, else the project's chief.
fn unwoken(core: &Core, agent_id: &str) -> Option<String> {
    core.names().reports_to(agent_id)?;
    let to = super::roster::hears_of(core, agent_id).agent()?.to_owned();
    let chat = core.chats().get(agent_id)?;
    let reply = chat.feed.last_answer()?;
    if !waits_in(&reply) || asks_in(&reply) {
        return None;
    }
    let now = core.agent_now(agent_id);
    let woken = chat.feed.running_tasks() > 0
        || chat.feed.running_subagents() > 0
        || chat.feed.has_queued()
        || now.waits_on_something()
        || now.workers_busy(agent_id)
        || super::turn_state::set_a_wake(core, agent_id);
    (!woken).then_some(to)
}

/// Whether the agent's last turn ended waiting on nothing set to wake it, and Sikemux sent it
/// back for that, in place of telling its lead.
pub(crate) fn sent_back_unwoken(core: &Core, agent_id: &str) -> bool {
    (core.harness().questions.state.locked().unwoken).get(agent_id) == Some(&false)
}

/// Whether the agent's last turn ended waiting on nothing set to wake it.
pub(crate) fn waits_unwoken(core: &Core, agent_id: &str) -> bool {
    (core.harness().questions.state.locked().unwoken).contains_key(agent_id)
}

/// Whether an agent's reply asks the person anything, in its own words.
pub(crate) fn asks_in(reply: &str) -> bool {
    !asked_in(reply).is_empty()
}

/// Whether the last paragraph of an agent's reply says it waits on something, or goes on once
/// something happens: "Still waiting on CI.", "I'll land once clippy passes."
pub(crate) fn waits_in(reply: &str) -> bool {
    let reply = reply.replace('\u{2019}', "'").to_lowercase();
    let last = last_paragraph(&reply)
        .replace(". ", "\n")
        .replace("! ", "\n");
    last.lines()
        .map(|sentence| sentence.trim_start_matches(['-', '*', '#', ' ', '>']))
        .any(says_it_waits)
}

fn last_paragraph(reply: &str) -> &str {
    reply.trim().rsplit("\n\n").next().unwrap_or_default()
}

fn says_it_waits(sentence: &str) -> bool {
    if sentence.starts_with("if ") {
        return false;
    }
    let padded = format!(" {} ", sentence.replace([',', ';', ':', '.', '*'], " "));
    let has = |words: &[&str]| words.iter().any(|word| padded.contains(word));
    let waiting_on = sentence
        .trim_start_matches("still ")
        .starts_with("waiting ")
        && has(&[" waiting for ", " waiting on ", " waiting until "]);
    let i_wait = has(&[
        " i'm waiting ",
        " i am waiting ",
        " i'm still waiting ",
        " i am still waiting ",
    ]);
    let goes_on_once = has(&[" i'll ", " i will "])
        && has(&[" once ", " when ", " as soon as ", " after ", " until "]);
    let still_going = has(&[
        " still compiling ",
        " still building ",
        " still running ",
        " still in progress ",
        " still queued ",
    ]) && !sentence.contains('@')
        && !sentence.contains("n't")
        && !has(&[" no ", " nothing ", " not "]);
    waiting_on || i_wait || goes_on_once || still_going
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
        let why = "Sent by Sikemux itself over a rule of this project, not by the person.";
        if let Err(error) = messages::send_back(&core, agent_id, text, why).await {
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
            let held = (state.open.iter())
                .filter(|open| question.holds.contains(&open.id))
                .map(|open| {
                    let given = AnswerParams {
                        id: open.id.clone(),
                        answer: Some(text.clone()),
                        paths: given.paths.clone(),
                        ..AnswerParams::default()
                    };
                    (open.clone(), given, text.clone())
                })
                .collect::<Vec<_>>();
            taken.push((question.clone(), given, text));
            taken.extend(held);
        }
        state
            .open
            .retain(|question| !taken.iter().any(|(taken, ..)| taken.id == question.id));
        for (question, given, text) in &taken {
            if let (None, None) = (&question.from_host, &question.dialog) {
                let text = match given.choice.is_some() {
                    true => without_attached(text, &given.paths),
                    false => text.clone(),
                };
                state.answered.push(Answered {
                    question: question.clone(),
                    answer: (!question.secret).then_some(text),
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
    let mut letters: Vec<Letter> = Vec::new();
    for (question, given, text) in taken {
        if let Some(host) = &question.from_host {
            let params = serde_json::to_value(&given).map_err(|error| error.to_string())?;
            let asked = serde_json::to_value(&question).map_err(|error| error.to_string())?;
            super::hosts::route::relay_answer(core, host, params, asked);
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
        let typed = (given.answer.clone())
            .map(|typed| without_attached(&typed, &given.paths))
            .filter(|typed| !typed.trim().is_empty());
        let told = match (&question.approval, question.secret, typed) {
            (Some(approval), ..) => super::approvals::settle(core, approval, given.choice)
                .await
                .map(|done| (text, Some(done), false)),
            (None, true, Some(typed)) => kept_privately(core, &question, typed).await.map(|note| {
                match picked_labels(&question, &given) {
                    Some(picked) => (format!("{picked}\n\n{note}"), None, true),
                    None => (note, None, true),
                }
            }),
            (None, ..) => Ok((text, None, false)),
        };
        match told {
            Ok((text, done, private)) => letters.push(Letter {
                question,
                text,
                done,
                paths: given.paths,
                private,
            }),
            Err(error) => {
                reopen(
                    letters
                        .into_iter()
                        .map(|letter| letter.question)
                        .chain([question])
                        .collect(),
                );
                return Err(error);
            }
        }
    }
    let mut askers: Vec<&str> = letters
        .iter()
        .map(|letter| letter.question.agent_id.as_str())
        .collect();
    askers.sort_unstable();
    askers.dedup();
    let mut failed = None;
    for asker in askers {
        let own: Vec<&Letter> = letters
            .iter()
            .filter(|letter| letter.question.agent_id == asker)
            .collect();
        let sent = own
            .iter()
            .map(|letter| {
                let Letter {
                    question,
                    text,
                    done,
                    paths,
                    private,
                } = letter;
                let note = done.clone().unwrap_or_else(|| match &question.about {
                    Some(about) if !question.holds.is_empty() => format!(
                        "The person answered the question you asked; @{} has the answer to its own question too. Go on with it.",
                        about.name
                    ),
                    _ => "The person answered the question you asked; go on with it.".to_owned(),
                });
                (
                    answered(question, &question.asked, *private),
                    text.clone(),
                    note,
                    paths.clone(),
                )
            })
            .collect();
        match messages::answer(core, asker.to_owned(), sent).await {
            Ok(delivered)
                if delivered.get("delivery").and_then(Value::as_str) == Some("waiting") => {}
            Ok(_) => core.harness().questions.delivered(asker),
            Err(error) => {
                reopen(
                    own.into_iter()
                        .map(|letter| letter.question.clone())
                        .collect(),
                );
                failed = Some(error);
                continue;
            }
        }
        for letter in own.into_iter().filter(|letter| !letter.private) {
            copy_to_lead(core, &letter.question, &letter.text).await;
        }
    }
    match failed {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

/// An answer as its asker is told it, and whether what the person typed went to a private file.
struct Letter {
    question: Question,
    text: String,
    done: Option<String>,
    paths: Vec<String>,
    private: bool,
}

/// What the person typed for a question whose answer is never kept, put in a file only their
/// account can open: what the asker is told in its place.
async fn kept_privately(
    core: &Arc<Core>,
    question: &Question,
    typed: String,
) -> Result<String, String> {
    let secret = tokio::task::spawn_blocking(move || sikemux_keychain::reference::resolve(&typed))
        .await
        .map_err(|error| error.to_string())
        .flatten()?;
    let path = core.harness().questions.keep_secret(question, &secret)?;
    Ok(secret_note(&path))
}

/// The labels of the choices picked in `given`, when it picked any.
fn picked_labels(question: &Question, given: &AnswerParams) -> Option<String> {
    let labels: Vec<&str> = (given.choice.iter())
        .filter_map(|&choice| Some(question.choices.get(choice)?.label.as_str()))
        .collect();
    (!labels.is_empty()).then(|| labels.join(", "))
}

/// The person's answer, copied to the lead a question went to first.
async fn copy_to_lead(core: &Arc<Core>, question: &Question, text: &str) {
    if let Stage::Person {
        lead: Some((lead_id, _)),
        ..
    } = &question.stage
    {
        let note = format!(
            "The person answered {}'s question, which went to you first. {} has the answer too.",
            question.name, question.name
        );
        let _ = messages::answer(
            core,
            lead_id.clone(),
            vec![(
                answered(question, &question.text, false),
                text.to_owned(),
                note,
                Vec::new(),
            )],
        )
        .await;
    }
}

fn answered(question: &Question, question_text: &str, private: bool) -> messages::Answered {
    messages::Answered {
        question: question_text.to_owned(),
        asker: question.name.clone(),
        asker_id: question.agent_id.clone(),
        cli: question.cli.clone(),
        secret: private,
    }
}

/// What the agent is told in place of the secret the person answered with.
fn secret_note(path: &Path) -> String {
    let path = path.display();
    format!(
        "The person's answer is a secret, so Sikemux kept it out of this conversation. It is in {path}, which only the person's account can read. Use it straight from the file, such as \"$(cat '{path}')\" in a command, and never print it, echo it, or write it into a reply, a message or another file."
    )
}

/// What the person's answer says: the choice they picked, with any words, or their words alone.
fn answer_text(question: &Question, params: &AnswerParams) -> Result<String, String> {
    match params.choice {
        Some(choice) => question
            .choices
            .get(choice)
            .map(|choice| with_picked(choice.label.clone(), params))
            .ok_or_else(|| "That choice is not one of the question's".into()),
        None => reply_text(params),
    }
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
                Some(
                    (given.choice.into_iter())
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
        let copied = std::fs::create_dir_all(dir).and_then(|()| std::fs::copy(from, &to));
        if let (Ok(_), Some(answer)) = (copied, answers.get_mut(at)) {
            answer.paths.push(to.to_string_lossy().into_owned());
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
    let unpicked = std::mem::take(&mut core.harness().questions.state.locked().unpicked);
    for (asker, note) in unpicked {
        let core = core.clone();
        tokio::spawn(async move {
            let _ = messages::from_sikemux(&core, asker, note).await;
        });
    }
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
        let held: HashSet<String> = (state.open.iter())
            .flat_map(|question| question.holds.iter().cloned())
            .collect();
        for question in &mut state.open {
            let Stage::Lead {
                lead_id,
                lead_name,
                ended_at,
            } = &mut question.stage
            else {
                continue;
            };
            if held.contains(&question.id) {
                continue;
            }
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
                };
                changed = true;
            } else if let Some(at) = ended_at.filter(|_| !unread) {
                next = sooner(next, Some(at + LEAD_GRACE_MS));
            }
        }
        if changed {
            core.harness().questions.changed(&mut state);
            core.boards().wake.now();
        }
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
            .filter(|question| question.listed_for_devices())
            .filter(|question| question.kind != Some(CallKind::Look))
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

/// The note on `agent`'s open calls that goes with the person's message to its chat.
pub(crate) fn open_calls(core: &Core, agent: &str) -> Option<sikemux_wire::protocol::ChatContext> {
    let text = core.harness().questions.open_calls_note(agent)?;
    Some(sikemux_wire::protocol::ChatContext {
        uri: format!("{}open-calls", sikemux_wire::protocol::FOR_AGENT_URI),
        title: "Your open calls".into(),
        text,
    })
}

#[cfg(test)]
mod tests {
    use super::Question;
    use sikemux_process::sync::Locked;

    #[tokio::test]
    async fn a_worker_stopped_for_its_leads_answer_waits_on_the_lead_rather_than_being_done() {
        use crate::server::activity::{Activity, ActivityCore};
        use crate::server::parts::Parts;
        use crate::server::{Core, ServerCore};
        use sikemux_view::turn::{TurnPhase, TurnWaitKind};

        let core = Core::new(Default::default(), None).unwrap();
        core.note_activity("worker", Activity::Started);
        let asked: Question = serde_json::from_value(serde_json::json!({
            "id": "question-1",
            "agentId": "worker",
            "name": "Garou",
            "cli": "opencode",
            "project": "widget",
            "asked": "Buy the display font for the welcome screen, $40?",
            "text": "Buy the display font for the welcome screen, $40?",
            "choices": [],
            "blocked": true,
            "assumption": null,
            "secret": false,
            "at": 7,
            "stage": { "stage": "lead", "leadId": "lead", "leadName": "Mumen", "endedAt": null },
        }))
        .expect("a question");
        core.harness().questions.state.locked().open.push(asked);
        core.note_activity("worker", Activity::Finished { cancelled: false });

        let turn = crate::server::turn_state::now(&core, "worker").expect("the turn ended");
        assert_eq!(turn.state, TurnPhase::Waiting);
        let wait = turn.wait.expect("what it waits on");
        assert_eq!(wait.kind, TurnWaitKind::Agent);
        assert_eq!(wait.what, "Mumen");
        assert_eq!(wait.started_at, 7);
    }

    /// A lead asked the person about its worker's question, then ended its turn, and the
    /// person got the worker's question as a second call.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_lead_asking_the_person_about_its_workers_question_raises_one_call() {
        use crate::server::parts::Parts;
        use crate::server::workspace::{AddProjectParams, AddProjectParamsExt};
        use crate::server::workspace_state::WorkspaceParts;
        use crate::server::{Core, ServerCore};

        let core = Core::new(Default::default(), None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let mut all = vec!["-c", "user.name=test", "-c", "user.email=test@localhost"];
            all.extend(args);
            crate::git::Git::at(dir.path()).text(&all).expect("git");
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["commit", "-q", "--allow-empty", "-m", "start"]);
        let project = crate::server::workspace::add_project(
            &core,
            AddProjectParams::at(dir.path().to_owned()),
        )
        .await
        .expect("the project");
        for agent in ["worker", "lead"] {
            let draft: serde_json::Map<String, serde_json::Value> =
                serde_json::from_value(serde_json::json!({
                    "id": agent,
                    "provider": "claude",
                    "project": project.id,
                    "cwd": dir.path(),
                    "title": agent,
                    "permissionMode": "auto",
                }))
                .expect("a draft");
            core.workspaces().put_agent_draft(draft).expect("the agent");
        }
        let workers: Question = serde_json::from_value(serde_json::json!({
            "id": "question-worker",
            "agentId": "worker",
            "name": "Nia",
            "cli": "claude",
            "project": "widget",
            "asked": "Which call page, A, B or C?",
            "text": "Which call page, A, B or C?",
            "choices": [],
            "blocked": true,
            "at": 7,
            "stage": { "stage": "lead", "leadId": "lead", "leadName": "Chief", "endedAt": 1 },
        }))
        .expect("a question");
        let mut leads = workers.clone();
        leads.id = "question-lead".to_owned();
        leads.agent_id = "lead".to_owned();
        leads.name = "Chief".to_owned();
        leads.stage = super::Stage::Person { lead: None };
        leads.about = Some(super::CallAsker {
            agent_id: "worker".to_owned(),
            name: "Nia".to_owned(),
            cli: "claude".to_owned(),
            chief: false,
        });
        let questions = &core.harness().questions;
        questions
            .state
            .locked()
            .open
            .extend([workers, leads.clone()]);

        super::hold_for(&core, &leads, None);
        super::settle(&core);

        let for_person: Vec<String> = questions.for_person().into_iter().map(|q| q.id).collect();
        assert_eq!(for_person, ["question-lead"]);
    }

    /// @Chief raised two calls about @Lille's two questions, and @Lille got the first call's
    /// answer for both.
    #[tokio::test(flavor = "multi_thread")]
    async fn two_calls_about_one_worker_each_answer_their_own_question() {
        use super::{AnswerParams, AnswersParams};
        use crate::server::parts::Parts;
        use crate::server::workspace::{AddProjectParams, AddProjectParamsExt};
        use crate::server::workspace_state::WorkspaceParts;
        use crate::server::{Core, ServerCore};

        let core = Core::new(Default::default(), None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let mut all = vec!["-c", "user.name=test", "-c", "user.email=test@localhost"];
            all.extend(args);
            crate::git::Git::at(dir.path()).text(&all).expect("git");
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["commit", "-q", "--allow-empty", "-m", "start"]);
        let project = crate::server::workspace::add_project(
            &core,
            AddProjectParams::at(dir.path().to_owned()),
        )
        .await
        .expect("the project");
        for agent in ["worker", "lead"] {
            let draft: serde_json::Map<String, serde_json::Value> =
                serde_json::from_value(serde_json::json!({
                    "id": agent,
                    "provider": "claude",
                    "project": project.id,
                    "cwd": dir.path(),
                    "title": agent,
                    "permissionMode": "auto",
                }))
                .expect("a draft");
            core.workspaces().put_agent_draft(draft).expect("the agent");
        }
        let workers = |id: &str, asked: &str| -> Question {
            serde_json::from_value(serde_json::json!({
                "id": id,
                "agentId": "worker",
                "name": "Lille",
                "cli": "claude",
                "project": project.id,
                "asked": asked,
                "text": asked,
                "choices": [],
                "blocked": true,
                "at": 7,
                "stage": { "stage": "lead", "leadId": "lead", "leadName": "Chief", "endedAt": null },
            }))
            .expect("a question")
        };
        let questions = &core.harness().questions;
        questions.state.locked().open.extend([
            workers("question-search", "Where does search go?"),
            workers("question-tasks", "T1 or T2 for teammates' tasks?"),
        ]);
        let mut calls = Vec::new();
        for (id, about, text) in [
            ("1", "question-search", "Where does search go?"),
            ("2", "question-tasks", "T1 or T2 for teammates' tasks?"),
        ] {
            let request = crate::cli::protocol::HarnessRequest {
                id: id.to_owned(),
                project: project.id.clone(),
                agent_id: Some("lead".to_owned()),
                method: "agent.ask".to_owned(),
                params: serde_json::json!({ "question": text, "about": about }),
            };
            let asked = super::ask(&core, &request).await.expect("the call");
            calls.push(asked["questionId"].as_str().unwrap().to_owned());
        }

        for (call, answer) in calls.iter().zip(["In the triggers view", "T1"]) {
            super::answer_all(
                &core,
                AnswersParams {
                    answers: vec![AnswerParams {
                        id: call.clone(),
                        answer: Some(answer.to_owned()),
                        ..AnswerParams::default()
                    }],
                },
            )
            .await
            .expect("the answer");
        }

        let answered: Vec<(String, Option<String>)> = (questions.state.locked().answered.iter())
            .filter(|answered| answered.question.agent_id == "worker")
            .map(|answered| (answered.question.id.clone(), answered.answer.clone()))
            .collect();
        assert_eq!(
            answered,
            [
                (
                    "question-search".to_owned(),
                    Some("In the triggers view".to_owned())
                ),
                ("question-tasks".to_owned(), Some("T1".to_owned())),
            ]
        );
    }

    /// An access call asking the person to press a button in a page only they reach kept its
    /// "Merged both" answer out of the chat, as if it were a password. Only a secret ask's is.
    #[tokio::test(flavor = "multi_thread")]
    async fn only_a_secret_asks_answer_stays_out_of_every_chat() {
        use crate::server::parts::Parts;
        use crate::server::{Core, ServerCore};
        use sikemux_view::calls::CallKind;

        let data = tempfile::tempdir().unwrap();
        let core = Core::new(Default::default(), Some(data.path())).unwrap();
        let ask = |id: &str, params: serde_json::Value| crate::cli::protocol::HarnessRequest {
            id: id.to_owned(),
            project: data.path().to_string_lossy().into_owned(),
            agent_id: Some("chief".to_owned()),
            method: "agent.ask".to_owned(),
            params,
        };
        let questions = &core.harness().questions;

        let press = ask(
            "1",
            serde_json::json!({
                "question": "Press Squash merge on both PRs?",
                "kind": "access",
                "choices": ["Merged both", "Wait"],
            }),
        );
        super::ask(&core, &press).await.expect("the access ask");
        let asked = questions.state.locked().open[0].clone();
        let call = asked.call();
        assert!(
            !asked.secret && !call.secret,
            "an access ask is not a secret"
        );
        assert_eq!(call.kind, CallKind::Access);
        assert_eq!(asked.view().secret_note, None);

        let password = ask(
            "2",
            serde_json::json!({ "question": "The staging password?", "secret": true }),
        );
        super::ask(&core, &password).await.expect("the secret ask");
        let asked = questions.state.locked().open[1].clone();
        let call = asked.call();
        assert!(asked.secret && call.secret);
        assert!(asked.view().secret_note.is_some());

        let typed = "hunter2-staging";
        questions.keep_secret(&asked, typed).expect("kept");
        let sent = questions.hide_secrets(&format!("Use {typed} to sign in."));
        assert_eq!(sent, format!("Use {} to sign in.", super::HIDDEN));
    }

    fn asked_in(reply: &str) -> Vec<(String, bool)> {
        (super::asked_in(reply).into_iter())
            .map(|ask| (ask.text, ask.polar))
            .collect()
    }

    #[test]
    fn a_worker_left_waiting_on_the_person_says_it_waits() {
        for said in [
            "Card #265 isn't part of my benchmark work, so I'm not taking it. Still waiting for the person to sign in on sanctum.",
            "I'm leaving card #266 for someone else; it's unrelated to the benchmark. Still waiting for the person to sign in on sanctum before I can start the build test.",
            "Fixed the probe.\n\nOnce that's done I'll check the setup and rerun the build test and the typing-lag test.",
        ] {
            assert!(super::waits_in(said), "{said}");
        }
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
