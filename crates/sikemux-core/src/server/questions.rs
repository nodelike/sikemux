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
use crate::text::{clipped, clipped_line};
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

mod answers;
mod escalation;
mod relay;
mod reply;
mod secrets;
mod store;
#[cfg(test)]
mod tests;

pub(crate) use answers::{answer, answer_all, lead_answered, views};
pub(crate) use escalation::{
    asks_in, offer_in, open_calls, prune_agents, reply_read, sent_back_unwoken, settle, turn_ended,
    waits_unwoken,
};
pub(crate) use relay::{
    answer_relayed, answer_turned_away, closed_elsewhere, relay_arrived, relayed_here,
};
pub(crate) use store::Questions;

use answers::*;
#[cfg(test)]
use escalation::waits_in;
use relay::*;
use reply::*;
use secrets::*;
use store::*;

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
        .map(|header| clipped_line(&header, MAX_HEADER_CHARS));
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
            Stage::Lead { lead_name, .. } => super::names::at_name(lead_name),
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
    if withdraw {
        if let Some(taken) = super::ship::take_back(core, agent_id, id) {
            taken?;
            return Ok(json!({ "withdrawn": id }));
        }
    }
    if let Some(reason) = text.as_deref().filter(|_| !withdraw) {
        if super::ship::explain_held(core, agent_id, id, reason) {
            return Ok(json!({ "edited": id }));
        }
    }
    let header = (text_param(params, "title", MAX_QUESTION_BYTES)?)
        .map(|header| clipped_line(&header, MAX_HEADER_CHARS));
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
            let mut held_withdrawn = Vec::new();
            if !question.holds.is_empty() {
                let held: HashSet<&str> = question.holds.iter().map(String::as_str).collect();
                state.open.retain(|open| {
                    if held.contains(open.id.as_str()) {
                        held_withdrawn.push(open.clone());
                        false
                    } else {
                        true
                    }
                });
            }
            questions.changed(&mut state);
            drop(state);
            if let Some(lead) = question.lead_id() {
                messages::withdraw(core, lead, &question.id);
            }
            for held in &held_withdrawn {
                if let Some(lead) = held.lead_id() {
                    messages::withdraw(core, lead, &held.id);
                }
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
    let index = core.harness().page_index.clone();
    let pages: Vec<String> = question.pages().map(|page| page.id.clone()).collect();
    if pages.is_empty() {
        return;
    }
    let call = question.id.clone();
    let linked = move || {
        index.link_calls(&pages, &call);
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
            header: Some(clipped_line(&header, MAX_HEADER_CHARS)),
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
            header: Some(clipped_line(header, MAX_HEADER_CHARS)),
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
                .map(|header| clipped_line(header, MAX_HEADER_CHARS));
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
        Some(Stage::Lead { lead_name, .. }) => {
            format!("{}, your lead,", super::names::at_name(lead_name))
        }
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
