//! Handing a chat that has begun to another agent, or forking it into a new chat. The agent
//! taking over starts a fresh session, and its first message carries what the chat said with the
//! person, chosen to fit a budget rather than written up by an agent: the first request, the
//! latest exchanges word for word, and how the commands run went. Every app shows it before then.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sikemux_process::sync::Locked;
use sikemux_view::chat::Handoff;
use sikemux_wire::protocol::{ChatContext, FOR_AGENT_URI};

use super::feed::Feed;
use crate::server::workspace::{AgentPatch, ChatChoice, UpdateAgentParams};
use crate::server::{Core, CoreError, CoreResult};

/// About how many tokens a handoff holds at most, leaving the agent taking over room to work.
const BUDGET_TOKENS: usize = 16_000;
/// The first request keeps this many characters at most; the latest exchanges take the rest.
const REQUEST_CHARS: usize = 8_000;
/// Each side of an exchange past this many characters is cut.
const LONGEST_SAID: usize = 6_000;
const COMMANDS: usize = 20;
const COMMAND_CHARS: usize = 200;

/// A handoff waiting for the chat's next message.
pub(crate) struct Pending {
    context: ChatContext,
    shown: Handoff,
}

/// The handoffs waiting, by the agent whose next message carries each.
#[derive(Default)]
pub(crate) struct Handoffs(std::sync::Mutex<HashMap<String, Pending>>);

impl Handoffs {
    fn set(&self, agent_id: &str, pending: Pending) {
        self.0.locked().insert(agent_id.to_owned(), pending);
    }

    /// What goes with `agent_id`'s next message, once.
    pub(crate) fn take(&self, agent_id: &str) -> Option<ChatContext> {
        self.0
            .locked()
            .remove(agent_id)
            .map(|pending| pending.context)
    }

    fn shown(&self, agent_id: &str) -> Option<Handoff> {
        (self.0.locked().get(agent_id)).map(|pending| pending.shown.clone())
    }

    pub(crate) fn forget(&self, agent_id: &str) -> bool {
        self.0.locked().remove(agent_id).is_some()
    }
}

fn tokens(chars: usize) -> u64 {
    chars.div_ceil(4) as u64
}

fn cut_start(text: &str, most: usize) -> String {
    let text = text.trim();
    let kept: String = text.chars().take(most).collect();
    match kept.len() < text.len() {
        true => format!("{kept} [cut]"),
        false => kept,
    }
}

fn cut_end(text: &str, most: usize) -> String {
    let text = text.trim();
    let skip = text.chars().count().saturating_sub(most);
    match skip {
        0 => text.to_owned(),
        _ => format!("[cut] {}", text.chars().skip(skip).collect::<String>()),
    }
}

/// What `feed` hands over from the chat it holds, which ran on `from`, or nothing when the
/// person has not said anything in it yet.
fn build(feed: &Feed, from: &str, source: &str, forked: bool) -> Option<Pending> {
    let (exchanges, commands) = feed.handed_over();
    let first = exchanges.first()?;
    let mut room = BUDGET_TOKENS * 4;
    let mut sections: Vec<String> = Vec::new();

    let ran: Vec<String> = commands
        .iter()
        .rev()
        .take(COMMANDS)
        .rev()
        .map(|(command, failed)| {
            let outcome = match failed {
                Some(false) => "succeeded",
                Some(true) => "failed",
                None => "did not finish",
            };
            let command = cut_start(&command.replace('\n', " "), COMMAND_CHARS);
            format!("- `{command}` {outcome}")
        })
        .collect();
    let ran = ran.join("\n");
    let request = cut_start(&first.0, REQUEST_CHARS);
    room = room.saturating_sub(ran.chars().count() + request.chars().count());

    let mut picked: Vec<String> = Vec::new();
    for (asked, answered) in exchanges.iter().rev() {
        let exchange = format!(
            "The person: {}\n\nThe agent: {}",
            cut_start(asked, LONGEST_SAID),
            cut_end(answered, LONGEST_SAID)
        );
        let Some(left) = room.checked_sub(exchange.chars().count() + 7) else {
            if picked.is_empty() {
                picked.push(exchange);
            }
            break;
        };
        room = left;
        picked.push(exchange);
    }
    picked.reverse();
    let left_out = exchanges.len() - picked.len();
    if left_out > 0 {
        sections.push(format!("## The person's first request\n\n{request}"));
    }
    sections.push(format!(
        "## The latest exchanges, word for word\n\n{}",
        picked.join("\n\n---\n\n")
    ));
    if !ran.is_empty() {
        sections.push(format!("## Commands run, newest last\n\n{ran}"));
    }

    let label = crate::agent_catalog::agent(from).map_or(from, |agent| agent.label);
    let opening = match forked {
        true => format!("This chat is a fork of one the person had with {label}."),
        false => format!("The person had this chat with {label} until now, and you take it over."),
    };
    let omitted = match left_out {
        0 => String::new(),
        1 => " One earlier exchange is left out.".to_owned(),
        count => format!(" {count} earlier exchanges are left out."),
    };
    let text = format!(
        "{opening} Below is what Sikemux picked from it to fit, not a summary.{omitted} \
         agent_read with agent {source} reads the whole chat. Its reasoning, tool results and \
         attachments are not carried over.\n\n{}",
        sections.join("\n\n")
    );
    let shown = Handoff {
        from: from.to_owned(),
        forked_from: forked.then(|| source.to_owned()),
        tokens: tokens(text.chars().count()),
        request: left_out > 0,
        exchanges: picked.len() as u32,
        left_out: left_out as u32,
        commands: commands.len().min(COMMANDS) as u32,
    };
    let context = ChatContext {
        uri: format!("{FOR_AGENT_URI}handoff"),
        title: format!("This chat so far, with {label}"),
        // A line that would close the block early is kept as text.
        text: text.replace("</context", "&lt;/context"),
    };
    Some(Pending { context, shown })
}

/// Tells the chat's apps what its next message hands over, or that nothing waits to be.
fn tell(core: &Core, agent_id: &str) {
    let Some(chat) = core.chats.get(agent_id) else {
        return;
    };
    let shown = core.chats.handoffs.shown(agent_id);
    super::offers::tell_update(&chat, sikemux_fold::update::handoff_pending(shown.as_ref()));
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HandOffParams {
    pub agent_id: String,
    /// The agent taking the chat over, such as `codex`.
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,
    /// A new chat takes it over, and this one stays as it is.
    #[serde(default)]
    pub fork: bool,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HandedOff {
    /// The chat that carries on: the same one, or the fork.
    pub agent_id: String,
    pub project: String,
}

/// Hands the chat to another agent, or forks it, and has the chat that carries on wait for the
/// person with the handoff shown.
pub(crate) async fn hand_off(core: &Arc<Core>, params: HandOffParams) -> CoreResult<HandedOff> {
    let source = core.workspaces.agent_or_gone(&params.agent_id)?;
    if !params.fork && core.chats.at_work(&params.agent_id) {
        return Err("Stop the chat's turn before handing it to another agent".into());
    }
    let feed = core.chats.readable(&params.agent_id)?;
    let pending = build(&feed, &source.provider, &source.id, params.fork)
        .ok_or_else(|| CoreError::from("This chat has nothing to hand over yet"))?;
    if params.fork {
        let launcher = match &params.profile_id {
            Some(profile) => format!("{}:{profile}", params.provider),
            None => params.provider.clone(),
        };
        let choice = ChatChoice {
            launcher,
            project: source.project.clone(),
            permission_mode: Some(source.permission_mode),
            model: params.model,
            effort: None,
            resume_id: None,
            cwd: source.worktree.is_some().then(|| source.cwd.clone()),
            title: None,
            terminal: false,
        };
        let (agent_id, _) = crate::server::launching::start_new(core, None, choice).await?;
        core.chats.handoffs.set(&agent_id, pending);
        tell(core, &agent_id);
        return Ok(HandedOff {
            agent_id,
            project: source.project,
        });
    }
    let agent_id = params.agent_id;
    if let Some(chat) = core.chats.get(&agent_id) {
        core.workspaces.note_stopping(&chat);
        if core.chats.stop(&agent_id) {
            core.chats.rest(&chat);
        }
    }
    let patch = AgentPatch {
        provider: Some(params.provider),
        profile_id: Some(params.profile_id),
        model: Some(params.model),
        model_provider: Some(params.model_provider),
        effort: Some(None),
        resume_id: Some(None),
        ..AgentPatch::default()
    };
    crate::server::workspace::update_agent(
        core,
        UpdateAgentParams {
            agent_id: agent_id.clone(),
            patch,
        },
    )?;
    core.chats.handoffs.set(&agent_id, pending);
    crate::server::launching::start_saved(core, &agent_id, true).await?;
    tell(core, &agent_id);
    Ok(HandedOff {
        agent_id,
        project: source.project,
    })
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DropHandoffParams {
    pub agent_id: String,
}

/// The chat's next message goes without the handoff waiting for it.
pub(crate) fn drop_handoff(core: &Core, params: DropHandoffParams) -> CoreResult<()> {
    if core.chats.handoffs.forget(&params.agent_id) {
        tell(core, &params.agent_id);
    }
    Ok(())
}

/// Tells a chat that hit its agent's usage limit on every account it may use which other agent
/// has room to take it over, while smart accounts is on.
pub(crate) fn offer_another_agent(chat: &super::Chat, launch: &sikemux_wire::protocol::ChatLaunch) {
    let Some(core) = chat.core.upgrade() else {
        return;
    };
    let book = core.workspaces.accounts().unwrap_or_default();
    if !book.smart {
        return;
    }
    let from = chat.provider();
    let Some(other) =
        crate::server::agent_accounts::other_agent_with_room(&book, from, &launch.cwd)
    else {
        return;
    };
    let label = |kind: &str| {
        crate::agent_catalog::agent(kind).map_or(kind.to_owned(), |agent| agent.label.to_owned())
    };
    let (from, to) = (label(from), label(other));
    let mut update = sikemux_fold::update::notice(
        "info",
        &format!("{to} has room to carry this chat on"),
        Some(&format!(
            "Every {from} account it may use is out of usage. Handing it to {to} takes your words with {from} along."
        )),
    );
    update["carryOnWith"] = serde_json::json!(other);
    super::offers::tell_update(chat, update);
}
