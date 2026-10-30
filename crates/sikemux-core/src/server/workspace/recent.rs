//! The saved chats devices can take up again: the recent ones read from each agent's own
//! history, and those the person pinned.

use super::*;

/// An agent's session a client takes up again, named by its agent and the project it ran in.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionChoice {
    pub provider: String,
    pub session_id: String,
    pub project: PathBuf,
    /// The worktree of `project` it ran in, when it ran in one.
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub title: Option<String>,
}

/// A recent chat's saved session, as its agent's CLI names it.
pub(crate) struct RecentSession {
    pub agent: AgentKind,
    pub cwd: String,
    pub session_id: String,
    /// The CLI the launcher that found it runs.
    pub program: Option<String>,
    /// The sign-in it was saved under; the agent's default when absent.
    pub config_path: Option<String>,
}

/// What a device asked of a recent chat it takes up again.
pub(crate) struct ResumeChoice {
    pub recent: String,
    pub permission_mode: Option<PermissionMode>,
    pub model: Option<String>,
    pub effort: Option<String>,
}

/// The recent chat named `recent`, or the pinned one when it left the recent ones.
pub(super) fn recent_or_pinned(published: &Published, recent: &str) -> Option<Recent> {
    let pinned = published
        .saved
        .pinned
        .iter()
        .map(|pin| pinned_chat(published, pin))
        .find(|chat| recent_id(chat) == recent);
    listed(published, recent).cloned().or(pinned)
}

/// The recent chat named `recent`, or one a device paged or searched to.
pub(super) fn listed<'a>(published: &'a Published, recent: &str) -> Option<&'a Recent> {
    published
        .recent
        .iter()
        .chain(&published.found)
        .find(|chat| recent_id(chat) == recent)
}

/// A pinned chat as a recent one that keeps no launcher.
pub(super) fn pinned_chat(published: &Published, pin: &PinnedChat) -> Recent {
    Recent {
        launcher: String::new(),
        provider: pin.provider.clone(),
        session_id: pin.session_id.clone(),
        title: pin.title.clone(),
        project: published
            .pin_homes
            .get(&pin.project)
            .unwrap_or(&pin.project)
            .clone(),
        cwd: pin.project.clone(),
        active_at: 0,
    }
}

/// A saved chat the core no longer lists, from its id and the folder it ran in.
pub(super) fn unlisted(recent: &str, cwd: PathBuf) -> Option<Recent> {
    let (provider, session_id) = recent.split_once(':')?;
    Some(Recent {
        launcher: String::new(),
        provider: provider.to_owned(),
        session_id: session_id.to_owned(),
        title: String::new(),
        project: cwd.clone(),
        cwd,
        active_at: 0,
    })
}

pub(super) fn recent_id(chat: &Recent) -> String {
    chat_id(&chat.provider, &chat.session_id)
}

pub(super) fn chat_id(provider: &str, session: &str) -> String {
    format!("{provider}:{session}")
}

/// Whether the agent of `provider` on `session` holds the recent chat.
pub(super) fn holds(chat: &Recent, provider: &str, session: Option<&str>) -> bool {
    provider == chat.provider && session == Some(chat.session_id.as_str())
}

/// The open chat that is the recent one.
pub(super) fn running_now<'a>(chat: &Recent, open: &'a [ChatInfo]) -> Option<&'a ChatInfo> {
    open.iter()
        .find(|live| holds(chat, &live.provider, live.session_id.as_deref()))
}

#[derive(Deserialize)]
pub(crate) struct PinnedParams {
    chats: Vec<PinnedChat>,
}

pub(crate) fn set_pinned_chats(core: &Core, params: PinnedParams) -> CoreResult<()> {
    core.workspaces.set_pinned(params.chats)
}

#[derive(Deserialize)]
pub(crate) struct PinParams {
    recent: String,
    pinned: bool,
}

pub(crate) fn pin_chat(core: &Core, params: PinParams) -> CoreResult<()> {
    core.workspaces.pin_chat(&params.recent, params.pinned)
}

/// The saved chats `chats` as a device sees them: those in a project on this host, each
/// with the agent holding it and whether the person pinned it.
pub(super) fn shown<'a>(
    published: &Published,
    chats: impl Iterator<Item = &'a Recent>,
    open: &[ChatInfo],
) -> Vec<RecentInfo> {
    let saved = &published.saved;
    let holder = |chat: &Recent| {
        running_now(chat, open)
            .map(|live| live.agent_id.clone())
            .or_else(|| {
                saved
                    .agents
                    .iter()
                    .find(|kept| holds(chat, &kept.agent.provider, kept.agent.resume_id.as_deref()))
                    .map(|kept| kept.agent.id.clone())
            })
    };
    let pinned = |chat: &Recent| {
        saved
            .pinned
            .iter()
            .any(|pin| pin.provider == chat.provider && pin.session_id == chat.session_id)
    };
    chats
        .filter_map(|chat| {
            let project = saved
                .projects
                .iter()
                .find(|project| project.path == chat.project)?;
            Some(RecentInfo {
                id: recent_id(chat),
                provider: chat.provider.clone(),
                title: chat.title.clone(),
                project: project.id.clone(),
                cwd: chat.cwd.clone(),
                active_at: chat.active_at,
                agent_id: holder(chat),
                pinned: pinned(chat),
            })
        })
        .collect()
}

/// The projects saved chats are looked for in.
pub(super) fn recent_projects(saved: &Saved) -> Vec<String> {
    saved
        .projects
        .iter()
        .take(saved_chats::MAX_PROJECTS)
        .map(|project| project.path.to_string_lossy().into_owned())
        .collect()
}

/// Each agent's saved chats are looked for once, on the account the person chose for it,
/// with the launcher that starts it there.
pub(super) fn recent_providers<'a>(
    launchers: impl Iterator<Item = &'a Launcher>,
    saved: &Saved,
) -> Vec<(String, RecentProvider)> {
    let selected = saved.accounts.as_ref().map(|book| &book.selected);
    let mut providers: Vec<(String, RecentProvider)> = Vec::new();
    for launcher in launchers {
        let Some(agent) = AgentKind::from_name(&launcher.agent.provider) else {
            continue;
        };
        let chosen = selected
            .and_then(|selected| selected.get(&launcher.agent.provider))
            .is_none_or(|profile| launcher.id.ends_with(&format!(":{profile}")));
        let taken = providers
            .iter()
            .any(|(_, provider)| provider.agent == agent);
        if chosen && !taken {
            let provider = RecentProvider {
                agent,
                config_path: launcher.agent.config_path.clone(),
            };
            providers.push((launcher.id.clone(), provider));
        }
    }
    providers
}

pub(super) fn saved_chat(launcher: String, session: saved_chats::RecentSession) -> Recent {
    Recent {
        launcher,
        provider: session.agent.to_owned(),
        session_id: session.id,
        title: session.title.chars().take(MAX_TITLE_CHARS).collect(),
        cwd: session
            .cwd
            .unwrap_or_else(|| session.project.clone())
            .into(),
        project: session.project.into(),
        active_at: session.mtime * 1000,
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecentChatsParams {
    /// Every project's when absent.
    project: Option<String>,
    query: Option<String>,
    cursor: Option<saved_chats::RecentCursor>,
    limit: Option<usize>,
}

#[derive(Serialize)]
pub(crate) struct RecentChats {
    chats: Vec<RecentInfo>,
    next: Option<saved_chats::RecentCursor>,
}

/// A page of a project's saved chats, or every project's, newest first, those matching
/// `query` when it has one. A device may take up any of them again as it does a recent one.
pub(crate) async fn recent_chats(core: &Core, asked: RecentChatsParams) -> CoreResult<RecentChats> {
    let (launchers, request) = core.workspaces.page_request(asked)?;
    let page = blocking(move || Ok(saved_chats::recent_page(request)?)).await?;
    let found = page
        .sessions
        .into_iter()
        .filter_map(|session| {
            let launcher = launchers
                .iter()
                .find(|launcher| launcher.split(':').next() == Some(session.agent))?;
            Some(saved_chat(launcher.clone(), session))
        })
        .collect();
    let chats = core.workspaces.note_found(found, &core.chats.list());
    Ok(RecentChats {
        chats,
        next: page.next,
    })
}

/// Looks for the newest saved chats whenever they may have changed, for
/// devices to take up again.
pub(crate) async fn recents(core: Arc<Core>) {
    loop {
        tokio::time::sleep(RECENT_SETTLE).await;
        let requests = core.workspaces.recent_requests();
        let pins = core.workspaces.pinned_away();
        let projects = core.workspaces.project_paths();
        let found = blocking(move || {
            let homes = pins
                .into_iter()
                .filter_map(|pin| Some((pin.clone(), home_of(&pin, &projects)?)))
                .collect();
            Ok((newest(requests), homes))
        })
        .await;
        if let Ok((found, homes)) = found {
            if core.workspaces.set_recent(found, homes) {
                core.view_changed();
            }
        }
        core.workspaces.recent_stale.notified().await;
    }
}

pub(super) fn newest(requests: Vec<(String, RecentRequest)>) -> Vec<Recent> {
    let mut found: Vec<Recent> = requests
        .into_iter()
        .flat_map(|(launcher, request)| {
            let page = saved_chats::recent_page(request).ok()?;
            Some(
                page.sessions
                    .into_iter()
                    .map(move |session| saved_chat(launcher.clone(), session)),
            )
        })
        .flatten()
        .collect();
    found.sort_by_key(|chat| std::cmp::Reverse(chat.active_at));
    found.truncate(RECENT_LIMIT);
    found
}

impl Workspaces {
    /// The saved chats may have changed, so the recent ones are looked for again.
    pub(crate) fn recent_stale(&self) {
        self.recent_stale.notify_one();
    }

    pub(super) fn set_pinned(&self, pinned: Vec<PinnedChat>) -> CoreResult<()> {
        let oversized = pinned.iter().any(|chat| {
            too_long(&chat.provider, 64)
                || too_long(&chat.session_id, MAX_FIELD_CHARS)
                || too_long(&chat.title, MAX_TITLE_CHARS)
        });
        if pinned.len() > MAX_PINNED || oversized {
            return Err("more pinned chats than the core keeps".into());
        }
        self.change(|saved| {
            saved.pinned = pinned;
            Ok(())
        })?;
        self.recent_stale();
        Ok(())
    }

    /// Pins or unpins the recent chat named `recent`, as `recent` lists it.
    pub(super) fn pin_chat(&self, recent: &str, pinned: bool) -> CoreResult<()> {
        let mut pins = self.published.locked().saved.pinned.clone();
        pins.retain(|pin| chat_id(&pin.provider, &pin.session_id) != recent);
        if pinned {
            let chat = listed(&self.published.locked(), recent)
                .map(|chat| PinnedChat {
                    provider: chat.provider.clone(),
                    session_id: chat.session_id.clone(),
                    title: chat.title.clone(),
                    project: chat.cwd.clone(),
                })
                .ok_or("that chat is not among the recent ones")?;
            pins.push(chat);
        }
        self.set_pinned(pins)
    }

    /// The saved session of the recent chat named `recent`, with the program and sign-in of
    /// the launcher that found it.
    pub(crate) fn recent_session(
        &self,
        recent: &str,
        cwd: Option<PathBuf>,
    ) -> CoreResult<RecentSession> {
        let published = self.published.locked();
        let chat = listed(&published, recent)
            .cloned()
            .or_else(|| unlisted(recent, cwd?))
            .ok_or("that chat is not among the recent ones")?;
        let agent = AgentKind::from_name(&chat.provider)
            .ok_or_else(|| CoreError::from(format!("{} keeps no saved chats", chat.provider)))?;
        let launcher = chat_launcher(&published, &chat);
        Ok(RecentSession {
            agent,
            cwd: chat.cwd.to_string_lossy().into_owned(),
            session_id: chat.session_id.clone(),
            program: launcher
                .and_then(|launcher| launcher.program.as_ref())
                .map(|program| program.to_string_lossy().into_owned()),
            config_path: launcher.and_then(|launcher| launcher.agent.config_path.clone()),
        })
    }

    /// The recent chats a device may take up again: those in a project on
    /// this host, leaving out any running now.
    pub(crate) fn recent(&self, open: &[ChatInfo]) -> Vec<RecentInfo> {
        let published = self.published.locked();
        let aged_out: Vec<Recent> = published
            .saved
            .pinned
            .iter()
            .filter(|pin| {
                !published
                    .recent
                    .iter()
                    .any(|chat| chat.provider == pin.provider && chat.session_id == pin.session_id)
            })
            .map(|pin| pinned_chat(&published, pin))
            .collect();
        shown(&published, published.recent.iter().chain(&aged_out), open)
    }

    /// How to look for a page of saved chats in the project `asked` names, or in every
    /// project, with the launcher that finds each agent's, terminal agents' too.
    pub(super) fn page_request(
        &self,
        asked: RecentChatsParams,
    ) -> CoreResult<(Vec<String>, RecentRequest)> {
        let published = self.published.locked();
        let saved = &published.saved;
        let projects = match &asked.project {
            Some(id) => vec![saved
                .project(id)
                .ok_or("that project is not on this host")?
                .path
                .to_string_lossy()
                .into_owned()],
            None => recent_projects(saved),
        };
        let (launchers, providers) = recent_providers(published.every_launcher(), &published.saved)
            .into_iter()
            .unzip();
        let request = RecentRequest {
            providers,
            projects,
            limit: asked.limit.unwrap_or(RECENT_PAGE),
            cursor: asked.cursor,
            query: asked.query,
            exclude: Vec::new(),
        };
        Ok((launchers, request))
    }

    /// Remembers the chats a device paged or searched to, so it may take them up again,
    /// and answers them as the device shows them.
    pub(super) fn note_found(&self, found: Vec<Recent>, open: &[ChatInfo]) -> Vec<RecentInfo> {
        let mut published = self.published.locked();
        published
            .found
            .retain(|kept| !found.iter().any(|chat| recent_id(chat) == recent_id(kept)));
        published.found.splice(0..0, found.iter().cloned());
        published.found.truncate(MAX_FOUND);
        shown(&published, found.iter(), open)
    }

    /// What to look for recent chats with: each chat agent's account once, in every project.
    /// Phones read these, and they start only chats.
    pub(super) fn recent_requests(&self) -> Vec<(String, RecentRequest)> {
        let published = self.published.locked();
        let projects = recent_projects(&published.saved);
        recent_providers(published.launchers.iter(), &published.saved)
            .into_iter()
            .map(|(launcher, provider)| {
                let request = RecentRequest {
                    providers: vec![provider],
                    projects: projects.clone(),
                    limit: RECENT_LIMIT,
                    cursor: None,
                    query: None,
                    exclude: Vec::new(),
                };
                (launcher, request)
            })
            .collect()
    }

    pub(super) fn set_recent(
        &self,
        recent: Vec<Recent>,
        pin_homes: HashMap<PathBuf, PathBuf>,
    ) -> bool {
        let mut published = self.published.locked();
        let changed = published.recent != recent || published.pin_homes != pin_homes;
        published.recent = recent;
        published.pin_homes = pin_homes;
        changed
    }

    /// The folders of pinned chats that are not a project's own, such as worktrees.
    pub(super) fn pinned_away(&self) -> Vec<PathBuf> {
        let published = self.published.locked();
        let saved = &published.saved;
        saved
            .pinned
            .iter()
            .map(|pin| &pin.project)
            .filter(|folder| {
                !saved
                    .projects
                    .iter()
                    .any(|project| &project.path == *folder)
            })
            .cloned()
            .collect()
    }

    /// How to start the recent chat a device chose, the way it would start a new one.
    pub(crate) fn resume_choice(
        &self,
        choice: ResumeChoice,
        open: &[ChatInfo],
    ) -> CoreResult<ChatChoice> {
        let published = self.published.locked();
        let chat = recent_or_pinned(&published, &choice.recent).ok_or_else(|| {
            CoreError::from("that chat is no longer among the recent ones on this host")
        })?;
        choice_of(&published, &chat, choice, open)
    }

    /// How to take up `provider`'s session again in the project at `project`, as a recent
    /// chat when it still is one.
    pub(crate) fn session_choice(
        &self,
        session: SessionChoice,
        open: &[ChatInfo],
    ) -> CoreResult<ChatChoice> {
        let published = self.published.locked();
        let recent = format!("{}:{}", session.provider, session.session_id);
        let chat = recent_or_pinned(&published, &recent).unwrap_or(Recent {
            launcher: String::new(),
            provider: session.provider,
            session_id: session.session_id,
            title: session.title.unwrap_or_default(),
            cwd: session.cwd.unwrap_or_else(|| session.project.clone()),
            project: session.project,
            active_at: 0,
        });
        let choice = ResumeChoice {
            recent,
            permission_mode: None,
            model: None,
            effort: None,
        };
        choice_of(&published, &chat, choice, open)
    }
}
