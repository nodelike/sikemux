use super::*;

impl Workspaces {
    pub fn agent_or_gone(&self, agent_id: &str) -> CoreResult<WorkspaceAgent> {
        self.agent(agent_id)
            .ok_or_else(|| CoreError::from("that agent is no longer on this host"))
    }

    /// The agent `agent_id` as it stood when it closed, while the workspace keeps it.
    pub fn closed_agent(&self, agent_id: &str) -> Option<WorkspaceAgent> {
        (self.published.locked().saved.closed.iter())
            .find(|agent| agent.id == agent_id)
            .cloned()
    }

    /// Lets go of the closed record of an agent that is open again.
    pub fn reopened(&self, agent_id: &str) {
        let kept = (self.published.locked().saved.closed.iter()).any(|agent| agent.id == agent_id);
        if kept {
            let _ = self.change(|saved| {
                saved.closed.retain(|agent| agent.id != agent_id);
                Ok(())
            });
        }
    }

    pub fn agent(&self, agent_id: &str) -> Option<WorkspaceAgent> {
        self.published
            .locked()
            .saved
            .kept(agent_id)
            .map(|kept| kept.agent.clone())
    }

    /// Adds an agent a client or another host describes, or replaces the one with its id.
    /// Without an id the core names it.
    pub fn put_agent_draft(&self, mut draft: Map<String, Value>) -> CoreResult<WorkspaceAgent> {
        if !draft.contains_key("id") {
            let id = crate::server::names::new_agent_id();
            draft.insert("id".into(), Value::String(id));
        }
        if !draft.contains_key("createdAt") {
            draft.insert("createdAt".into(), epoch_ms().into());
        }
        let agent: WorkspaceAgent = serde_json::from_value(Value::Object(draft))
            .map_err(|error| CoreError::from(format!("that is not an agent: {error}")))?;
        self.put_agent(agent)
    }

    /// Adds an agent, or replaces the one with its id.
    pub fn put_agent(&self, mut agent: WorkspaceAgent) -> CoreResult<WorkspaceAgent> {
        let put = self.change(|saved| {
            if agent.buddy.is_none() {
                agent.buddy = saved
                    .kept(&agent.id)
                    .and_then(|kept| kept.agent.buddy.clone());
            }
            dress(&mut agent, &saved.agents);
            check_agent(&agent, saved)?;
            let count = saved.agents.len();
            match saved.kept_mut(&agent.id) {
                Some(kept) => kept.agent = agent.clone(),
                None if count >= MAX_AGENTS => {
                    return Err("this host already keeps as many agents as it can".into())
                }
                None => saved.agents.push(Kept {
                    agent: agent.clone(),
                    account: None,
                    fallbacks: Vec::new(),
                    accounts: BTreeMap::new(),
                    program: None,
                    held: false,
                    moved: false,
                    place: ListPlace::default(),
                }),
            }
            Ok(agent)
        })?;
        Ok(put)
    }

    /// Changes the agent `agent_id` as `edit` does. Clients hear of it once
    /// the change is kept, so callers need not tell them.
    pub fn edit_agent(
        &self,
        agent_id: &str,
        edit: impl FnOnce(&mut WorkspaceAgent),
    ) -> CoreResult<WorkspaceAgent> {
        self.change(|saved| {
            let previous = saved
                .kept(agent_id)
                .ok_or_else(|| CoreError::from("that agent is not on this host"))?;
            let mut agent = previous.agent.clone();
            let previous_provider = previous.agent.provider.clone();
            let previous_profile = previous
                .agent
                .profile_id
                .clone()
                .or_else(|| previous.account.as_ref().map(|account| account.id.clone()));
            let remembered = previous.accounts.clone();
            edit(&mut agent);
            let provider_changed = agent.provider != previous_provider;
            let profile_changed = agent.profile_id != previous.agent.profile_id;
            if provider_changed && agent.profile_id.is_none() {
                if let Some(id) = remembered.get(&agent.provider) {
                    let book = saved.accounts.clone().unwrap_or_default();
                    if by_hand(&agent.provider) || book.can_keep(&agent.provider, id, &agent.cwd) {
                        agent.profile_id = Some(id.clone());
                    }
                }
            }
            dress(&mut agent, &saved.agents);
            check_agent(&agent, saved)?;
            if let Some(kept) = saved.kept_mut(agent_id) {
                if provider_changed {
                    if let Some(profile) = previous_profile {
                        kept.accounts.insert(previous_provider, profile);
                    }
                    kept.program = None;
                }
                if provider_changed || profile_changed {
                    kept.account = None;
                    kept.fallbacks.clear();
                }
                kept.agent = agent.clone();
            }
            Ok(agent)
        })
    }

    /// Gives the agent the title its chat earned, unless the person named it.
    /// A first prompt's words stand in only while the agent has no title of
    /// its own, and a command never does. Answers whether the title changed.
    pub(super) fn note_title(&self, agent_id: &str, title: &str, stand_in: bool) -> bool {
        if title.starts_with('/') || title.starts_with('<') {
            return false;
        }
        let title: String = match stand_in {
            true => crate::text::clipped(title, STAND_IN_TITLE_CHARS),
            false => title.to_owned(),
        };
        self.change(|saved| {
            let Some(kept) = saved.kept(agent_id) else {
                return Ok(false);
            };
            let agent = &kept.agent;
            let untitled = untitled(saved, agent);
            if agent.renamed || agent.title == title || (stand_in && !untitled) {
                return Ok(false);
            }
            // A chief is always titled by its role, never by its greeting, prompt or CLI title.
            let chief =
                (saved.projects.iter()).any(|project| project.chief.as_deref() == Some(agent_id));
            if chief {
                return Ok(false);
            }
            if let Some(kept) = saved.kept_mut(agent_id) {
                kept.agent.title = title;
            }
            Ok(true)
        })
        .unwrap_or(false)
    }

    /// The agent's title, once it has one of its own rather than its agent's or account's name.
    pub fn own_title(&self, agent_id: &str) -> Option<String> {
        let published = self.published.locked();
        let agent = &published.saved.kept(agent_id)?.agent;
        (agent.renamed || !untitled(&published.saved, agent)).then(|| agent.title.clone())
    }

    /// Holds every agent asleep until the person, or an agent they set going
    /// again, opens or messages it, as the person stops everything.
    pub fn hold_all(&self) {
        let _ = self.change(|saved| {
            for kept in &mut saved.agents {
                kept.held = true;
            }
            Ok(())
        });
    }

    /// Whether the agent waits to be opened or messaged by the person, or by an
    /// agent they set going again.
    pub fn held(&self, agent_id: &str) -> bool {
        (self.published.locked().saved.kept(agent_id)).is_some_and(|kept| kept.held)
    }

    /// Whether the agent moved to another account it has yet to open its chat on.
    pub fn moved(&self, agent_id: &str) -> bool {
        (self.published.locked().saved.kept(agent_id)).is_some_and(|kept| kept.moved)
    }

    pub fn note_moved(&self, agent_id: &str, moved: bool) {
        if self.moved(agent_id) != moved {
            let _ = self.change(|saved| {
                if let Some(kept) = saved.kept_mut(agent_id) {
                    kept.moved = moved;
                }
                Ok(())
            });
        }
    }

    /// The agent started, so mail waiting for it wakes it again when it sleeps.
    pub fn woke(&self, agent_id: &str) {
        if self.held(agent_id) {
            let _ = self.change(|saved| {
                if let Some(kept) = saved.kept_mut(agent_id) {
                    kept.held = false;
                }
                Ok(())
            });
        }
    }

    pub fn set_asleep(&self, agent_id: &str, asleep: bool) {
        let _ = self.change(|saved| {
            if let Some(kept) = saved.kept_mut(agent_id) {
                kept.agent.asleep = asleep;
            }
            Ok(())
        });
    }

    pub(super) fn close_agents(&self, agent_ids: &[String]) -> CoreResult<()> {
        self.change(|saved| {
            let (closing, open) = std::mem::take(&mut saved.agents)
                .into_iter()
                .partition(|kept: &Kept| agent_ids.contains(&kept.agent.id));
            saved.agents = open;
            for kept in closing {
                saved.closed.retain(|agent| agent.id != kept.agent.id);
                saved.closed.insert(0, kept.agent);
            }
            saved.closed.truncate(MOST_CLOSED);
            Ok(())
        })?;
        self.recent_stale();
        Ok(())
    }

    /// Every agent in the workspace, running or not.
    /// The account each agent in the workspace runs on, by profile id.
    pub fn agent_runs(&self) -> Vec<(String, Option<String>, bool)> {
        (self.published.locked().saved.agents.iter())
            .map(|kept| {
                let agent = &kept.agent;
                (agent.id.clone(), agent.profile_id.clone(), agent.terminal)
            })
            .collect()
    }

    pub fn agents(&self) -> Vec<WorkspaceAgent> {
        (self.published.locked().saved.agents.iter())
            .map(|kept| kept.agent.clone())
            .collect()
    }

    pub fn agent_ids(&self) -> std::collections::HashSet<String> {
        self.published
            .locked()
            .saved
            .agents
            .iter()
            .map(|kept| kept.agent.id.clone())
            .collect()
    }

    /// Names each session's project by id: an agent's own project, else the
    /// deepest project folder holding the folder it started in.
    pub fn place(&self, sessions: &mut [SessionInfo]) {
        let published = self.published.locked();
        let saved = &published.saved;
        for session in sessions {
            let agent = session.agent_id.as_deref().and_then(|id| saved.kept(id));
            session.project = match agent {
                Some(kept) => Some(kept.agent.project.clone()),
                None => session.project.as_deref().and_then(|folder| {
                    saved
                        .projects
                        .iter()
                        .filter(|project| Path::new(folder).starts_with(&project.path))
                        .max_by_key(|project| project.path.components().count())
                        .map(|project| project.id.clone())
                }),
            };
        }
    }

    /// Each worker's lead, by the worker's id.
    pub fn agent_leads(&self) -> HashMap<String, String> {
        self.published
            .locked()
            .saved
            .agents
            .iter()
            .filter_map(|kept| Some((kept.agent.id.clone(), kept.agent.lead.clone()?)))
            .collect()
    }

    /// The agent each one is listed under, by its id: a worker's lead, a side chat's main chat.
    pub fn agent_parents(&self) -> HashMap<String, String> {
        let published = self.published.locked();
        let agents = published.saved.agents.iter().map(|kept| &kept.agent);
        agents
            .filter_map(|agent| {
                let parent = agent.lead.as_ref().or(agent.side_of.as_ref())?;
                Some((agent.id.clone(), parent.clone()))
            })
            .collect()
    }

    pub fn agent_projects(&self) -> HashMap<String, String> {
        self.published
            .locked()
            .saved
            .agents
            .iter()
            .map(|kept| (kept.agent.id.clone(), kept.agent.project.clone()))
            .collect()
    }

    /// Keeps a chat that just started among the agents, so every client
    /// lists it whoever started it: a device, an agent, Auto or the app.
    pub fn note_begun(
        &self,
        launch: &ChatLaunch,
        started_by: Option<&str>,
        launcher: Option<&str>,
    ) {
        if let Some(known) = self.agent(&launch.agent_id) {
            let unnamed = known.profile_id.is_none() && launch.account.is_some();
            if known.asleep || known.terminal || unnamed {
                let _ = self.change(|saved| {
                    if let Some(kept) = saved.kept_mut(&known.id) {
                        (kept.agent.asleep, kept.agent.terminal) = (false, false);
                        if kept.agent.profile_id.is_none() {
                            kept.agent.profile_id =
                                launch.account.as_ref().map(|account| account.id.clone());
                        }
                    }
                    Ok(())
                });
            }
            self.woke(&known.id);
            return self.note_program(launch, launcher);
        }
        let lead = started_by.and_then(|lead| self.agent(lead));
        let project = match &lead {
            Some(lead) => lead.project.clone(),
            None => match self.project_for(&launch.cwd) {
                Ok(project) => project,
                Err(error) => {
                    eprintln!("sikemux core: a chat that started was not kept: {error}");
                    return;
                }
            },
        };
        let profile_id = launcher
            .map(LauncherId::parse)
            .filter(|launcher| launcher.kind == launch.provider)
            .and_then(|launcher| launcher.account)
            .map(str::to_owned)
            .or_else(|| launch.account.as_ref().map(|account| account.id.clone()))
            .or_else(|| {
                let keeps = crate::agent_catalog::agent(&launch.provider)?.accounts;
                let book = self.accounts().unwrap_or_default();
                keeps.then(|| book.chosen(&launch.provider))
            });
        let worktree = lead.as_ref().and_then(|lead| {
            let worktree = lead.worktree.as_ref()?;
            launch
                .cwd
                .starts_with(&worktree.path)
                .then(|| worktree.clone())
        });
        let agent = WorkspaceAgent {
            id: launch.agent_id.clone(),
            name: None,
            provider: launch.provider.clone(),
            project,
            cwd: launch.cwd.clone(),
            title: launch.provider.clone(),
            renamed: false,
            titled: false,
            lead: lead.map(|lead| lead.id),
            side_of: None,
            resume_id: launch.resume_id.clone(),
            permission_mode: launch.permission_mode,
            profile_id,
            model: launch.model.clone(),
            model_provider: launch.model_provider.clone(),
            effort: launch.effort.clone(),
            tags: Vec::new(),
            keep_alive: false,
            asleep: false,
            worktree,
            buddy: None,
            face: None,
            pty_id: None,
            terminal: false,
            created_at: epoch_ms(),
        };
        if let Err(error) = self.put_agent(agent) {
            eprintln!("sikemux core: a chat that started was not kept: {error}");
        }
        self.note_program(launch, launcher);
    }

    /// Keeps the program a chat runs when a client chose it rather than a launcher, so a wake
    /// starts that same program and never one found on its own.
    pub(super) fn note_program(&self, launch: &ChatLaunch, launcher: Option<&str>) {
        let program = launcher.is_none().then(|| launch.program.clone());
        let _ = self.change(|saved| {
            if let Some(kept) = saved.kept_mut(&launch.agent_id) {
                kept.program = program;
            }
            Ok(())
        });
    }

    /// Keeps how the agent's chat runs as it stops, so it can be woken on its
    /// saved conversation with no app there.
    pub fn note_stopping(&self, agent_id: &str, launch: ChatLaunch) {
        let _ = self.change(|saved| {
            if let Some(kept) = saved.kept_mut(agent_id) {
                let agent = &mut kept.agent;
                agent.resume_id = launch.resume_id.or(agent.resume_id.take());
                agent.permission_mode = launch.permission_mode;
                agent.model = launch.model;
                agent.effort = launch.effort;
                kept.account = launch.account;
                kept.fallbacks = launch.fallbacks;
            }
            Ok(())
        });
        self.recent_stale();
    }

    /// How the agent's chat was last running, to start it again.
    pub fn kept_launch(&self, agent_id: &str) -> Option<Kept> {
        self.published.locked().saved.kept(agent_id).cloned()
    }

    /// The launcher and mode an agent runs with.
    pub fn run_of(&self, agent_id: &str) -> Option<AgentRun> {
        self.agent(agent_id).map(|agent| AgentRun {
            launcher: launcher_of(&agent),
            permission_mode: agent.permission_mode,
        })
    }

    pub fn title(&self, agent_id: &str) -> Option<String> {
        self.agent(agent_id).as_ref().and_then(named)
    }

    /// The running chats under the workspace's titles, which include the
    /// agent's own unless the person renamed it, then the chat agents that are
    /// not running.
    pub fn listed(&self, mut running: Vec<ChatInfo>) -> Vec<ChatInfo> {
        let published = self.published.locked();
        let agents = &published.saved.agents;
        for chat in &mut running {
            let title = agents
                .iter()
                .find(|kept| kept.agent.id == chat.agent_id)
                .and_then(|kept| named(&kept.agent));
            if title.is_some() {
                chat.title = title;
            }
        }
        let stopped: Vec<ChatInfo> = agents
            .iter()
            .map(|kept| &kept.agent)
            .filter(|agent| is_chat_provider(&agent.provider) && !agent.terminal)
            .filter(|agent| !running.iter().any(|live| live.agent_id == agent.id))
            .map(|agent| ChatInfo {
                agent_id: agent.id.clone(),
                name: None,
                provider: agent.provider.clone(),
                title: named(agent),
                cwd: agent.cwd.clone(),
                session_id: None,
                state: if agent.asleep {
                    ChatState::Asleep
                } else {
                    ChatState::Stopped
                },
                running: false,
                pending_permissions: Vec::new(),
                subagents: 0,
                tasks: 0,
                reconnecting: None,
                started_by: None,
                launcher: None,
                permission_mode: agent.permission_mode,
                model: None,
                model_provider: None,
                effort: None,
                unread: false,
                active_at: None,
                group: Default::default(),
                status: Default::default(),
                next_trigger: None,
                called_at: None,
                attention_rank: 0,
            })
            .collect();
        running.extend(stopped);
        running
    }

    pub fn closed(&self, agent_id: &str) -> bool {
        self.agent(agent_id).is_none()
    }

    /// Whether the agent runs in its CLI's own terminal rather than as a chat.
    pub fn in_terminal(&self, agent_id: &str) -> bool {
        self.agent(agent_id).is_some_and(|agent| agent.terminal)
    }

    pub fn published(&self, agent_id: &str) -> Option<bool> {
        self.agent(agent_id).map(|agent| agent.asleep)
    }
}

/// The agent's title when it has one beyond its CLI's name.
pub(super) fn named(agent: &WorkspaceAgent) -> Option<String> {
    Some(agent.title.clone()).filter(|title| !title.trim().is_empty() && title != &agent.provider)
}

/// An agent new to the workspace, with nothing yet of what its running adds.
pub fn new_agent(
    id: String,
    provider: String,
    project: String,
    cwd: PathBuf,
    permission_mode: PermissionMode,
) -> WorkspaceAgent {
    WorkspaceAgent {
        id,
        name: None,
        title: provider.clone(),
        provider,
        project,
        cwd,
        renamed: false,
        titled: false,
        lead: None,
        side_of: None,
        resume_id: None,
        permission_mode,
        profile_id: None,
        model: None,
        model_provider: None,
        effort: None,
        tags: Vec::new(),
        keep_alive: false,
        asleep: false,
        worktree: None,
        buddy: None,
        face: None,
        pty_id: None,
        terminal: false,
        created_at: epoch_ms(),
    }
}

pub(super) fn is_chat_provider(provider: &str) -> bool {
    crate::agent_catalog::agent(provider).is_some_and(|agent| agent.chat.is_some())
}

pub(super) fn buddy_colour(agent: &WorkspaceAgent) -> Option<usize> {
    let colour = agent.buddy.as_ref()?.color?;
    (colour <= MAX_BUDDY_COLOUR).then_some(colour as usize)
}

/// Gives an agent without a buddy colour the one its neighbours in the same
/// folder wear least, then the one fewest agents wear, so agents side by side
/// rarely match on any client.
pub(super) fn dress(agent: &mut WorkspaceAgent, others: &[Kept]) {
    if buddy_colour(agent).is_some() {
        return;
    }
    let mut scores = [0u32; BUDDY_COLOURS];
    for other in others.iter().map(|kept| &kept.agent) {
        let Some(score) = buddy_colour(other).and_then(|colour| scores.get_mut(colour)) else {
            continue;
        };
        if other.id != agent.id {
            *score += if other.cwd == agent.cwd { 1000 } else { 1 };
        }
    }
    let least = (scores.iter().enumerate())
        .min_by_key(|(_, score)| **score)
        .map_or(0, |(slot, _)| slot);
    agent.buddy.get_or_insert_default().color = Some(least as u32);
}

pub(super) fn check_agent(agent: &WorkspaceAgent, saved: &Saved) -> CoreResult<()> {
    let long = |text: &Option<String>| {
        text.as_deref()
            .is_some_and(|text| too_long(text, MAX_FIELD_CHARS))
    };
    fn detail(value: &Option<impl Serialize>) -> bool {
        value.as_ref().is_some_and(|value| {
            serde_json::to_vec(value).map_or(true, |bytes| bytes.len() > MAX_DETAIL_BYTES)
        })
    }
    if !is_tool_agent_id(&agent.id) {
        return Err("that is not an agent id".into());
    }
    if too_long(&agent.title, MAX_TITLE_CHARS)
        || too_long(&agent.provider, 64)
        || [
            &agent.lead,
            &agent.resume_id,
            &agent.profile_id,
            &agent.model,
            &agent.model_provider,
            &agent.effort,
        ]
        .into_iter()
        .any(long)
        || detail(&agent.worktree)
        || detail(&agent.buddy)
    {
        return Err("the agent has a field longer than the core keeps".into());
    }
    if !agent.cwd.is_absolute() {
        return Err("an agent's folder must be an absolute path".into());
    }
    if saved.project(&agent.project).is_none() {
        return Err("that project is not on this host".into());
    }
    Ok(())
}

#[derive(Deserialize)]
pub struct CreateAgentParams {
    agent: Map<String, Value>,
}

/// Adds an agent to the workspace, or replaces the one with its id.
pub fn create_agent(core: &Core, mut params: CreateAgentParams) -> CoreResult<WorkspaceAgent> {
    let draft = &mut params.agent;
    if let (None, Some(Value::String(session))) = (draft.get("id"), draft.get("resumeId")) {
        let provider = draft.get("provider").and_then(Value::as_str);
        let id = (core.names()).returning_id(provider, session, core.open_agents());
        draft.insert("id".into(), Value::String(id));
    }
    core.workspaces().put_agent_draft(params.agent)
}

pub trait AgentPatchExt: Sized {
    fn apply(self, agent: &mut WorkspaceAgent);
}

impl AgentPatchExt for AgentPatch {
    fn apply(self, agent: &mut WorkspaceAgent) {
        fn set<T>(field: &mut T, to: Option<T>) {
            if let Some(to) = to {
                *field = to;
            }
        }
        set(&mut agent.title, self.title);
        set(&mut agent.renamed, self.renamed);
        set(&mut agent.provider, self.provider);
        set(&mut agent.permission_mode, self.permission_mode);
        set(&mut agent.keep_alive, self.keep_alive);
        set(&mut agent.tags, self.tags.map(tidy_tags));
        set(&mut agent.lead, self.lead);
        set(&mut agent.resume_id, self.resume_id);
        set(&mut agent.profile_id, self.profile_id);
        set(&mut agent.model, self.model);
        set(&mut agent.model_provider, self.model_provider);
        set(&mut agent.effort, self.effort);
        set(&mut agent.buddy, self.buddy);
    }
}

/// Tags as they are kept: trimmed, lower case, spaces as single spaces, each once.
pub(super) fn tidy_tags(tags: Vec<String>) -> Vec<String> {
    let mut kept: Vec<String> = Vec::new();
    for tag in tags {
        let tag = tag
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        if !tag.is_empty() && !kept.contains(&tag) {
            kept.push(tag);
        }
    }
    kept
}

/// Changes some of an agent's fields; a null one is cleared.
pub fn update_agent(core: &Core, params: UpdateAgentParams) -> CoreResult<WorkspaceAgent> {
    core.workspaces()
        .edit_agent(&params.agent_id, |agent| params.patch.apply(agent))
}

pub fn close_agents(core: &Core, params: CloseAgentsParams) -> CoreResult<()> {
    close(core, &params.agent_ids)
}

/// Closes every agent left from before a fresh start, unless the person keeps their tabs
/// open across one. Their chats stay in Recent, with their triggers paused.
pub fn restore(core: &Core) {
    if core.preferences().get().restore_agent_tabs {
        return;
    }
    let agents: Vec<String> = core.workspaces().agent_ids().into_iter().collect();
    if let Err(error) = close(core, &agents) {
        eprintln!("sikemux core: the agents from before stayed open: {error}");
    }
}

/// Stops the agent and takes it out of the workspace, so every app closes it.
pub fn close_agent(core: &Core, agent_id: String) -> CoreResult<()> {
    close(core, &[agent_id])
}

pub fn close(core: &Core, agent_ids: &[String]) -> CoreResult<()> {
    for agent in agent_ids
        .iter()
        .filter_map(|id| core.workspaces().agent(id))
    {
        core.names()
            .note_look(&agent.id, agent.buddy.clone(), named(&agent));
        (core.workspace_above().agent_closing)(core, &agent);
    }
    let trees: Vec<_> = (agent_ids.iter())
        .filter_map(|id| core.workspaces().agent(id)?.worktree)
        .collect();
    let before = core.workspaces().agent_ids();
    core.workspaces().close_agents(agent_ids)?;
    for agent_id in agent_ids {
        (core.workspace_above().agent_closed)(core, agent_id);
    }
    agents_closed(core, before);
    (core.workspace_above().worktrees_closed)(core, agent_ids, trees);
    Ok(())
}

/// Whether the agent has no title of its own yet, only its agent's or account's name.
pub(super) fn untitled(saved: &Saved, agent: &WorkspaceAgent) -> bool {
    let profile = |id: &str| {
        saved
            .accounts
            .as_ref()?
            .profiles
            .iter()
            .find(|profile| profile.id == id)
            .map(|profile| profile.name.clone())
    };
    agent.title.trim().is_empty()
        || agent.title == agent.provider
        || agent.profile_id.as_deref().and_then(profile).as_deref() == Some(&agent.title)
}

pub fn retitle(core: &Core, agent_id: &str, title: Option<String>, stand_in: bool) {
    if let Some(title) = title {
        if core.workspaces().note_title(agent_id, &title, stand_in) {
            (core.workspace_above().title_moved)(core);
        }
        if let Some(own) = core.workspaces().own_title(agent_id).filter(|_| !stand_in) {
            (core.workspace_above().chat_titled)(core, agent_id, &own);
        }
    }
}

/// Takes out of the workspace agents that have nothing left to show.
pub fn close_gone(core: &Core, agent_ids: &[String]) {
    let _ = close(core, agent_ids);
}

/// What follows agents leaving the workspace, however they left: their chats
/// and the tasks they started stop, and their names are free again.
pub(super) fn agents_closed(core: &Core, before: HashSet<String>) {
    let kept = core.workspaces().agent_ids();
    let gone: Vec<&String> = before.difference(&kept).collect();
    (core.workspace_above().agents_gone)(core, &gone);
    let forgotten = core.drafts().forget(
        gone.iter()
            .map(|agent_id| sikemux_view::chat::chat_draft_key(agent_id)),
    );
    if !forgotten.is_empty() {
        crate::server::drafts::changed(core, forgotten);
    }
    settle_names(core);
}

/// Agents in the workspace keep their names and the accounts they run on, and the names of
/// agents gone from it are free again.
pub(super) fn settle_names(core: &Core) {
    core.names().note_runs(core.workspaces().agent_runs());
    core.names().settle(core.open_agents());
}
