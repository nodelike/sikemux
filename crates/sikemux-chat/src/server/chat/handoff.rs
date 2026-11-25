//! Handing a chat that has begun to another agent, or forking it into a new chat. The agent
//! taking over starts a fresh session, and its first message carries what the chat said with the
//! person, chosen to fit a budget rather than written up by an agent: the first request, the
//! latest exchanges word for word, and how the commands run went. Every app shows it before then.

use crate::server::chat_state::ChatParts;
use crate::server::workspace_state::WorkspaceParts;
pub use sikemux_calls::chat::{ChatHandedOff, DropHandoffParams, HandOffParams};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::chat::{Handoff, HandoffSide};
use sikemux_wire::protocol::ChatContext;

use super::feed::{Feed, HandedOver};
use crate::agent_accounts::by_hand;
use crate::agent_catalog::AgentKind;
use crate::server::workspace::{AgentPatch, ChatChoice, UpdateAgentParams};
use crate::server::{Core, CoreError, CoreResult};
use crate::sessions::transcript::{self, Said};
use sikemux_view::workspace::WorkspaceAgent;

/// About how many tokens a handoff holds at most, leaving the agent taking over room to work.
const BUDGET_TOKENS: usize = 16_000;
/// The first request keeps this many characters at most; the latest exchanges take the rest.
const REQUEST_CHARS: usize = 8_000;
/// Each side of an exchange past this many characters is cut.
const LONGEST_SAID: usize = 6_000;
const COMMANDS: usize = 20;
const COMMAND_CHARS: usize = 200;
const FILES: usize = 40;
const FILES_CHARS: usize = 1_500;
const WORK_CHARS: usize = 4_000;
const DECISIONS: usize = 40;
const DECISION_DAYS: u64 = 14;
const DECISIONS_CHARS: usize = 6_000;
const RULES_CHARS: usize = 3_000;
const NOTE_CHARS: usize = 2_000;
const LINE_CHARS: usize = 600;
/// The latest turns read from a sleeping chat's saved session.
const SESSION_TURNS: usize = 60;

/// A handoff waiting for the chat's next message.
#[derive(Clone, Serialize, Deserialize)]
pub struct Pending {
    context: ChatContext,
    /// What the apps show of it; none for a fresh session's brief, which the chat's line shows.
    shown: Option<Handoff>,
    /// The fresh session a brief was built for, the only one that takes it.
    session: Option<String>,
    /// The model and agent the chat is handed over from.
    #[serde(default)]
    from: Option<HandoffSide>,
    /// What the chat ran on before it was handed over, which switching back to resumes.
    #[serde(default)]
    back: Option<RanOn>,
}

/// The agent, account, model and session a chat ran on.
#[derive(Clone, Serialize, Deserialize)]
struct RanOn {
    provider: String,
    profile_id: Option<String>,
    model: Option<String>,
    model_provider: Option<String>,
    effort: Option<String>,
    resume_id: Option<String>,
}

impl RanOn {
    fn of(agent: &WorkspaceAgent) -> Self {
        Self {
            provider: agent.provider.clone(),
            profile_id: agent.profile_id.clone(),
            model: agent.model.clone(),
            model_provider: agent.model_provider.clone(),
            effort: agent.effort.clone(),
            resume_id: agent.resume_id.clone(),
        }
    }
}

/// The handoffs waiting, by the agent whose next message carries each, each also on disk so a
/// core that starts after this one still sends what its apps show.
#[derive(Default)]
pub struct Handoffs {
    folder: Option<PathBuf>,
    pending: std::sync::Mutex<HashMap<String, Pending>>,
}

impl Handoffs {
    pub fn new(data_dir: Option<&Path>) -> Self {
        let folder = data_dir.map(|dir| dir.join("handoffs"));
        let mut pending = HashMap::new();
        let files = (folder.as_deref()).and_then(|folder| std::fs::read_dir(folder).ok());
        for file in files.into_iter().flatten().flatten() {
            let path = file.path();
            let agent = path.file_stem().and_then(|stem| stem.to_str());
            let read = std::fs::read(&path).ok();
            let kept = read.and_then(|bytes| serde_json::from_slice::<Pending>(&bytes).ok());
            if let (Some(agent), Some(kept)) = (agent, kept) {
                pending.insert(agent.to_owned(), kept);
            }
        }
        Self {
            folder,
            pending: std::sync::Mutex::new(pending),
        }
    }

    fn file(&self, agent_id: &str) -> Option<PathBuf> {
        Some(self.folder.as_ref()?.join(format!("{agent_id}.json")))
    }

    fn set(&self, agent_id: &str, pending: Pending) {
        if let Some(file) = self.file(agent_id) {
            let written = (file.parent().map_or(Ok(()), std::fs::create_dir_all)).and_then(|()| {
                std::fs::write(&file, serde_json::to_vec(&pending).unwrap_or_default())
            });
            if let Err(error) = written {
                eprintln!("sikemux core: a handoff was not kept on disk: {error}");
            }
        }
        self.pending.locked().insert(agent_id.to_owned(), pending);
    }

    /// Has `agent_id`'s next message carry the brief of the fresh `session` it goes to.
    pub fn carry(&self, agent_id: &str, context: ChatContext, session: &str) {
        let pending = Pending {
            context,
            shown: None,
            session: Some(session.to_owned()),
            from: None,
            back: None,
        };
        self.set(agent_id, pending);
    }

    /// What goes with `agent_id`'s next message, in `session`, once, and the model and agent
    /// it was handed over from. A brief built for another session is let go.
    pub fn take(
        &self,
        agent_id: &str,
        session: &str,
    ) -> Option<(ChatContext, Option<HandoffSide>)> {
        let pending = self.pending.locked().remove(agent_id)?;
        self.unkeep(agent_id);
        (pending.session.as_deref())
            .is_none_or(|built_for| built_for == session)
            .then_some((pending.context, pending.from))
    }

    fn back(&self, agent_id: &str) -> Option<(RanOn, Option<HandoffSide>)> {
        let pending = self.pending.locked();
        let pending = pending.get(agent_id)?;
        Some((pending.back.clone()?, pending.from.clone()))
    }

    pub fn shown(&self, agent_id: &str) -> Option<Handoff> {
        (self.pending.locked().get(agent_id)).and_then(|pending| pending.shown.clone())
    }

    pub fn forget(&self, agent_id: &str) -> bool {
        let had = self.pending.locked().remove(agent_id).is_some();
        if had {
            self.unkeep(agent_id);
        }
        had
    }

    fn unkeep(&self, agent_id: &str) {
        if let Some(file) = self.file(agent_id) {
            let _ = std::fs::remove_file(file);
        }
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

fn shell_command(title: &str) -> Option<&str> {
    for prefix in [
        "Bash ",
        "shell ",
        "exec_command ",
        "shell_command ",
        "run_command ",
    ] {
        if let Some(cmd) = title.strip_prefix(prefix) {
            let cmd = cmd.trim();
            if !cmd.is_empty() {
                return Some(cmd);
            }
        }
    }
    None
}

fn bounded_lines(lines: &[String], max_items: usize, max_chars: usize) -> String {
    let mut out = String::new();
    let mut used = 0usize;
    let total = lines.len();
    let mut shown = 0usize;
    for raw in lines.iter().take(max_items) {
        let line = cut_start(raw, LINE_CHARS);
        let len = line.chars().count() + 1;
        if used + len > max_chars {
            break;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&line);
        used += len;
        shown += 1;
    }
    if shown < total && shown > 0 {
        out.push_str(&format!("\n- and {} more", total - shown));
    }
    out
}

#[derive(Default)]
struct WorkspaceSnapshot {
    modified_files: Vec<String>,
    open_work: Vec<String>,
    decisions: Vec<String>,
    rules: Option<String>,
    handoff_note: Option<String>,
}

fn snapshot_for(core: &Core, project: &str, agent_id: &str, cwd: &Path) -> WorkspaceSnapshot {
    let modified_files: Vec<String> =
        (core.chat_above().handoff_files)(core, project, agent_id, cwd)
            .into_iter()
            .map(|file| format!("- `{file}`"))
            .collect();
    let open_work = (core.chat_above().open_work)(core, agent_id);
    let since = epoch_ms().saturating_sub(DECISION_DAYS * 24 * 60 * 60_000);
    let decisions: Vec<String> =
        (core.chat_above().board_standing_since)(core, project, since, DECISIONS)
            .into_iter()
            .map(|decision| match &decision.why {
                Some(why) => format!("- #{} {} ({why})", decision.id, decision.says()),
                None => format!("- #{} {}", decision.id, decision.says()),
            })
            .collect();
    let rules = (core.chat_above().memory_after_compaction)(core, agent_id);
    let handoff_note = core
        .chats()
        .get(agent_id)
        .and_then(|chat| chat.rotation.handoff())
        .or_else(|| {
            core.chats()
                .sessions
                .kept(agent_id)
                .and_then(|kept| kept.handoff())
        });
    WorkspaceSnapshot {
        modified_files,
        open_work,
        decisions,
        rules,
        handoff_note,
    }
}

fn snapshot(core: &Core, source: &WorkspaceAgent) -> WorkspaceSnapshot {
    snapshot_for(core, &source.project, &source.id, &source.cwd)
}

/// What the chat said, from the feed the core holds of it, or for a chat asleep without one, from
/// its agent's saved session.
fn said(core: &Core, agent: &WorkspaceAgent) -> Option<HandedOver> {
    if let Ok(feed) = core.chats().readable(&agent.id) {
        return Some(feed.handed_over());
    }
    let session = agent.resume_id.as_deref()?;
    let kind = AgentKind::from_name(&agent.provider)?;
    let book = core.workspaces().accounts().unwrap_or_default();
    let homes = (book.with_built_ins().profiles.into_iter())
        .filter(|profile| profile.provider == agent.provider)
        .filter_map(|profile| profile.config_path.map(PathBuf::from));
    let path = homes
        .map(Some)
        .chain([None])
        .find_map(|home| transcript::locate(kind, home.as_deref(), session))?;
    let latest = transcript::read(kind, &path, None, SESSION_TURNS).ok()?;
    let first = transcript::read(kind, &path, Some(0), 1).ok();
    let first_ask = first.and_then(|first| {
        first.said.into_iter().find_map(|(_, _, said)| match said {
            Said::Asked(text) => Some(text),
            _ => None,
        })
    });
    let mut exchanges: Vec<(String, String)> = Vec::new();
    let mut commands: Vec<(String, String, Option<bool>)> = Vec::new();
    for (_, _, said) in latest.said {
        match said {
            Said::Asked(text) => exchanges.push((text.trim().to_owned(), String::new())),
            Said::Message(text) => {
                if let Some((_, answer)) = exchanges.last_mut() {
                    answer.push_str(&text);
                }
            }
            Said::Tool { id, title, failed } => match title {
                Some(title) => {
                    if let Some(cmd) = shell_command(&title) {
                        commands.push((id, cmd.to_owned(), Some(failed)));
                    }
                }
                None if failed => {
                    if let Some((_, _, outcome)) =
                        commands.iter_mut().rev().find(|(each, ..)| *each == id)
                    {
                        *outcome = Some(true);
                    }
                }
                None => {}
            },
            _ => {}
        }
    }
    if let Some(first_ask) = first_ask.map(|text| text.trim().to_owned()) {
        if exchanges
            .first()
            .is_none_or(|(asked, _)| *asked != first_ask)
        {
            exchanges.insert(0, (first_ask, String::new()));
        }
    }
    exchanges.retain(|(asked, answered)| !asked.is_empty() || !answered.trim().is_empty());
    let commands = commands
        .into_iter()
        .map(|(_, cmd, failed)| (cmd, failed))
        .collect();
    Some((exchanges, commands))
}

/// What the chat hands over from what it said, which ran on `from`, or nothing when the person
/// has not said anything in it yet.
fn build(
    said: HandedOver,
    workspace: &WorkspaceSnapshot,
    from: &str,
    source: &str,
    forked: bool,
) -> Option<Pending> {
    let (exchanges, commands) = said;
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
    let files = bounded_lines(&workspace.modified_files, FILES, FILES_CHARS);
    let work = bounded_lines(&workspace.open_work, usize::MAX, WORK_CHARS);
    let decisions = bounded_lines(&workspace.decisions, DECISIONS, DECISIONS_CHARS);
    let rules = workspace
        .rules
        .as_deref()
        .map(|rules| cut_start(rules, RULES_CHARS))
        .unwrap_or_default();
    let handoff_note = workspace
        .handoff_note
        .as_deref()
        .map(|note| cut_start(note, NOTE_CHARS))
        .unwrap_or_default();
    let request = cut_start(&first.0, REQUEST_CHARS);
    room = room.saturating_sub(
        ran.chars().count()
            + files.chars().count()
            + work.chars().count()
            + decisions.chars().count()
            + rules.chars().count()
            + handoff_note.chars().count()
            + request.chars().count(),
    );

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
    if !rules.is_empty() {
        sections.push(format!("## Active rules and standing policy\n\n{rules}"));
    }
    if !handoff_note.is_empty() {
        sections.push(format!(
            "## Previous session's handoff note\n\n{handoff_note}"
        ));
    }
    if left_out > 0 {
        sections.push(format!("## The person's first request\n\n{request}"));
    }
    sections.push(format!(
        "## The latest exchanges, word for word\n\n{}",
        picked.join("\n\n---\n\n")
    ));
    if !files.is_empty() {
        sections.push(format!("## Modified files\n\n{files}"));
    }
    if !work.is_empty() {
        sections.push(format!("## Work in hand\n\n{work}"));
    }
    if !decisions.is_empty() {
        sections.push(format!("## Recent decisions\n\n{decisions}"));
    }
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
    let title = format!("This chat so far, with {label}");
    let context = crate::sessions::for_agent::sikemux_context("handoff", &title, &text);
    Some(Pending {
        context,
        shown: Some(shown),
        session: None,
        from: None,
        back: None,
    })
}

/// `provider`'s `model`, or the model its chat runs on now, named as the chat's model picker
/// names it.
pub fn side(feed: Option<&Feed>, provider: &str, model: Option<&str>) -> HandoffSide {
    let picker = (feed.and_then(Feed::config_options))
        .and_then(|options| sikemux_fold::pickers::pickers(&options).0);
    let value = (model.map(str::to_owned)).or_else(|| picker.as_ref().map(|p| p.current.clone()));
    let choice = (picker.as_ref().zip(value.as_deref()))
        .and_then(|(picker, value)| picker.choices.iter().find(|choice| choice.value == value));
    let maker = (choice.and_then(|choice| choice.maker.clone())).or_else(|| {
        crate::agent_catalog::agent(provider)
            .and_then(|agent| agent.maker)
            .map(str::to_owned)
    });
    HandoffSide {
        agent: provider.to_owned(),
        model: choice.map(|choice| choice.label.clone()).or(value),
        maker,
    }
}

/// Tells the chat's apps what its next message hands over, or that nothing waits to be.
fn tell(core: &Core, agent_id: &str) {
    if let Some(feed) = core.chats().heard(agent_id) {
        feed.set_handoff(core.chats().handoffs.shown(agent_id));
    }
}

/// Hands the chat to another agent, or forks it, and has the chat that carries on wait for the
/// person with the handoff shown. A chat that has said nothing yet just moves.
pub async fn hand_off(core: &Arc<Core>, params: HandOffParams) -> CoreResult<ChatHandedOff> {
    let source = core.workspaces().agent_or_gone(&params.agent_id)?;
    if !params.fork && core.chats().at_work(&params.agent_id) {
        return Err("Stop the chat's turn before handing it to another agent".into());
    }
    let earlier = (!params.fork)
        .then(|| core.chats().handoffs.back(&params.agent_id))
        .flatten();
    if let Some((back, _)) = earlier.as_ref().filter(|(back, _)| {
        back.provider == params.provider
            && (params.profile_id.is_none() || params.profile_id == back.profile_id)
    }) {
        return go_back(core, source, back.clone(), params).await;
    }
    let said = said(core, &source)
        .ok_or_else(|| CoreError::from("Sikemux no longer has what this chat said"))?;
    let workspace = snapshot(core, &source);
    let ran_on = (earlier.as_ref()).map_or(source.provider.as_str(), |(back, _)| &back.provider);
    let pending = build(said, &workspace, ran_on, &source.id, params.fork);
    if params.fork {
        let pending =
            pending.ok_or_else(|| CoreError::from("This chat has nothing to hand over yet"))?;
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
        let agent_id = (core.chat_above().start_new)(core, choice).await?;
        core.chats().handoffs.set(&agent_id, pending);
        tell(core, &agent_id);
        return Ok(ChatHandedOff {
            agent_id,
            project: source.project,
        });
    }
    let agent_id = params.agent_id;
    let (back, from) = match earlier {
        Some((back, from)) => (back, from),
        None => (
            RanOn::of(&source),
            Some(side(
                core.chats().readable(&agent_id).ok().as_deref(),
                &source.provider,
                source.model.as_deref(),
            )),
        ),
    };
    let patch = AgentPatch {
        provider: Some(params.provider),
        profile_id: Some(params.profile_id),
        model: Some(params.model),
        model_provider: Some(params.model_provider),
        effort: Some(None),
        resume_id: Some(None),
        ..AgentPatch::default()
    };
    let pending = pending.map(|pending| Pending {
        from,
        back: Some(back),
        ..pending
    });
    restart(core, agent_id, patch, pending, source.project).await
}

/// Puts a chat handed over but not yet written to back on the agent and session it ran on, with
/// nothing left to hand over.
async fn go_back(
    core: &Arc<Core>,
    source: WorkspaceAgent,
    back: RanOn,
    params: HandOffParams,
) -> CoreResult<ChatHandedOff> {
    let book = core.workspaces().accounts().unwrap_or_default();
    let profile_id = back
        .profile_id
        .filter(|id| by_hand(&back.provider) || book.can_keep(&back.provider, id, &source.cwd));
    let patch = AgentPatch {
        provider: Some(back.provider),
        profile_id: Some(profile_id),
        model: Some(params.model.or(back.model)),
        model_provider: Some(params.model_provider.or(back.model_provider)),
        effort: Some(back.effort),
        resume_id: Some(back.resume_id),
        ..AgentPatch::default()
    };
    restart(core, params.agent_id, patch, None, source.project).await
}

/// Stops the chat, moves it onto what `patch` says with `pending` waiting for its next message,
/// and starts it again.
async fn restart(
    core: &Arc<Core>,
    agent_id: String,
    patch: AgentPatch,
    pending: Option<Pending>,
    project: String,
) -> CoreResult<ChatHandedOff> {
    core.chats().resets.cancel(&agent_id);
    if let Some(chat) = core.chats().get(&agent_id) {
        core.workspaces()
            .note_stopping(chat.agent_id(), chat.current_now());
        core.chats().stop(&agent_id);
    }
    crate::server::workspace::update_agent(
        core,
        UpdateAgentParams {
            agent_id: agent_id.clone(),
            patch,
        },
    )?;
    match pending {
        Some(pending) => core.chats().handoffs.set(&agent_id, pending),
        None => {
            core.chats().handoffs.forget(&agent_id);
        }
    }
    (core.chat_above().start_saved)(core, &agent_id, true).await?;
    tell(core, &agent_id);
    Ok(ChatHandedOff { agent_id, project })
}

/// Builds a full handoff context when a resumed chat had to start a fresh vendor session
/// (for example after an Antigravity account switch or a missing Codex thread).
pub fn for_fresh_session(core: &Core, chat: &super::Chat) -> Option<ChatContext> {
    let said = chat.feed.handed_over();
    let project = core.workspaces().project_of(&chat.launch.cwd);
    let workspace = snapshot_for(core, &project, chat.agent_id(), &chat.launch.cwd);
    let pending = build(said, &workspace, chat.provider(), chat.agent_id(), false)?;
    Some(pending.context)
}

/// The chat's next message goes without the handoff waiting for it.
pub fn drop_handoff(core: &Core, params: DropHandoffParams) -> CoreResult<()> {
    if core.chats().handoffs.forget(&params.agent_id) {
        tell(core, &params.agent_id);
    }
    Ok(())
}

/// Tells a chat that hit its agent's usage limit on every account it may use which other agent
/// has room to take it over, while smart accounts is on.
pub fn offer_another_agent(chat: &super::Chat, launch: &sikemux_wire::protocol::ChatLaunch) {
    let Some(core) = chat.core.upgrade() else {
        return;
    };
    let book = core.workspaces().accounts().unwrap_or_default();
    if !book.smart {
        return;
    }
    let from = chat.provider();
    // Antigravity's chat may still go on with another of its own accounts, which it offers.
    if by_hand(from) && !launch.fallbacks.is_empty() {
        return;
    }
    let Some(other) = (core.chat_above().other_agent_with_room)(&book, from, &launch.cwd) else {
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
    if let Some(fields) = update.as_object_mut() {
        fields.insert("carryOnWith".to_owned(), serde_json::json!(other));
    }
    super::offers::tell_update(chat, update);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handoff_carries_rules_handoff_note_and_6kb_decisions_budget() {
        let decisions: Vec<String> = (1..=30)
            .map(|i| {
                format!(
                    "- #{i} Standing architecture decision number {i} covering cross-harness memory durability and routing."
                )
            })
            .collect();
        let workspace = WorkspaceSnapshot {
            modified_files: vec!["- `crates/sikemux-core/src/server/memory/brief.rs`".into()],
            open_work: vec!["- Your worker @Sabito: working on memory durability".into()],
            decisions,
            rules: Some("Rules:\n- #1 Consult Gemini 4 Neon, never Fable.".into()),
            handoff_note: Some("Finishing Wave 1 verification before landing Commit 1.".into()),
        };
        let said = (
            vec![("switch to antigravity".into(), "switched cleanly".into())],
            Vec::new(),
        );
        let pending = build(said, &workspace, "claude", "chief-1", false).expect("handoff built");
        let text = &pending.context.text;
        assert!(text.contains("## Active rules and standing policy"));
        assert!(text.contains("Consult Gemini 4 Neon, never Fable"));
        assert!(text.contains("## Previous session's handoff note"));
        assert!(text.contains("Finishing Wave 1 verification"));
        assert!(text.contains("## Recent decisions"));
        // More than the old 1,500-char (~15 short lines / ~5 normal lines) cap must fit
        assert!(text.contains("#25 Standing architecture decision"));
    }
}
