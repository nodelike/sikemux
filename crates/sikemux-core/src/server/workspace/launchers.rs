//! What the host offers to start agents with: each launcher, the accounts and model providers
//! behind it, and the launch a chat gets from them.

use super::*;

/// The launcher that starts `agent`: its CLI on the account it runs on.
pub(crate) fn launcher_of(agent: &WorkspaceAgent) -> String {
    match &agent.profile_id {
        Some(profile) => format!("{}:{profile}", agent.provider),
        None => agent.provider.clone(),
    }
}

/// The launcher, of those offered as (id, agent kind), that starts agents of `kind`: the one on the
/// account the person chose for them, else the first of that kind.
pub(crate) fn launcher_for<'a>(
    offered: impl Iterator<Item = (&'a str, &'a str)> + Clone,
    accounts: Option<&Book>,
    kind: &str,
) -> Option<String> {
    let chosen = accounts
        .and_then(|book| book.selected.get(kind))
        .map(|account| format!("{kind}:{account}"));
    let mut of_kind = offered.filter(|(_, provider)| *provider == kind);
    chosen
        .filter(|id| of_kind.clone().any(|(offered, _)| offered == id))
        .or_else(|| of_kind.next().map(|(id, _)| id.to_owned()))
}

/// The launcher that found `chat`. A chat that left the recent ones keeps no launcher, so
/// it takes the account the person chose for its agent.
pub(super) fn chat_launcher<'a>(published: &'a Published, chat: &Recent) -> Option<&'a Launcher> {
    let chosen = published
        .saved
        .accounts
        .as_ref()
        .map(|book| format!("{}:{}", chat.provider, book.chosen(&chat.provider)));
    published
        .every_launcher()
        .filter(|launcher| launcher.agent.provider == chat.provider)
        .find(|launcher| {
            launcher.id == chat.launcher
                || (chat.launcher.is_empty() && Some(&launcher.id) == chosen.as_ref())
        })
        .or_else(|| {
            chat.launcher.is_empty().then(|| {
                published
                    .every_launcher()
                    .find(|launcher| launcher.agent.provider == chat.provider)
            })?
        })
}

/// How to start `chat` again on the launcher it last ran on, or the account the person
/// chose for its agent when it keeps none.
pub(super) fn choice_of(
    published: &Published,
    chat: &Recent,
    choice: ResumeChoice,
    open: &[ChatInfo],
) -> CoreResult<ChatChoice> {
    let saved = &published.saved;
    if running_now(chat, open).is_some() {
        return Err("that chat is already open on this host".into());
    }
    let project = saved
        .projects
        .iter()
        .find(|project| project.path == chat.project)
        .ok_or_else(|| CoreError::from("that chat's project is not on this host"))?;
    let launcher = chat_launcher(published, chat).ok_or_else(|| {
        CoreError::from("Sikemux on this host can no longer start that chat's agent")
    })?;
    Ok(ChatChoice {
        launcher: launcher.id.clone(),
        project: project.id.clone(),
        permission_mode: choice.permission_mode,
        model: choice.model,
        effort: choice.effort,
        resume_id: Some(chat.session_id.clone()),
        cwd: (chat.cwd != chat.project).then(|| chat.cwd.clone()),
        title: Some(chat.title.clone()).filter(|title| !title.trim().is_empty()),
    })
}

pub(crate) struct ChatChoice {
    pub launcher: String,
    pub project: String,
    pub permission_mode: Option<PermissionMode>,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// The provider's session to load instead of opening a new one.
    pub resume_id: Option<String>,
    /// The worktree of the project it resumes in, when it ran in one.
    pub cwd: Option<PathBuf>,
    /// The title the chat it resumes already had.
    pub title: Option<String>,
}

/// A launcher as devices see it, with the models and effort its last session offered.
pub(super) fn launcher_info(saved: &Saved, launcher: &Launcher) -> LauncherInfo {
    let config_options = saved
        .launcher_options
        .get(&launcher.id)
        .cloned()
        .unwrap_or(Value::Null);
    let (model, effort) = pickers::pickers(&config_options);
    LauncherInfo {
        id: launcher.id.clone(),
        provider: launcher.agent.provider.clone(),
        label: launcher.label.clone(),
        permission_mode: launcher.permission_mode.clone(),
        config_options,
        status: launcher
            .status
            .as_ref()
            .map(|status| status.state().to_owned()),
        model,
        effort,
    }
}

/// An account the agent picker offers whose agent reports its plan usage.
pub(crate) struct UsageAccount {
    pub launcher: String,
    pub label: String,
    pub provider: String,
    pub config_path: Option<String>,
    pub signed_in: bool,
}

/// Finds each offered agent's program and offers those found, with what
/// the core last heard of their sign-in. Runs as the core starts and when the
/// person's accounts or their mode change, or they may have installed an agent.
pub(crate) fn refresh_launchers(core: Arc<Core>) -> impl std::future::Future<Output = ()> {
    let asked = core.workspaces.asked.fetch_add(1, Ordering::SeqCst) + 1;
    async move { refresh(core, asked).await }
}

pub(super) async fn refresh(core: Arc<Core>, asked: u64) {
    let looking: Vec<_> = core
        .workspaces
        .offered()
        .into_iter()
        .map(|launcher| {
            tokio::spawn(async move {
                let program = match status::find(&launcher.agent).await {
                    Ok(program) => program,
                    Err(status::AgentStatus::Broken { reason }) => {
                        return Err((launcher.id, reason))
                    }
                    Err(_) => return Err((launcher.id, "not found".to_owned())),
                };
                let agent = &launcher.agent;
                let status = status::cached(&agent.provider, agent.config_path.as_deref());
                Ok(Launcher {
                    status,
                    program: Some(program),
                    ..launcher
                })
            })
        })
        .collect();
    let mut found = Vec::with_capacity(looking.len());
    let mut missed = BTreeMap::new();
    for looked in looking {
        match looked.await {
            Ok(Ok(launcher)) => found.push(launcher),
            Ok(Err((id, reason))) => {
                missed.insert(id, reason);
            }
            Err(_) => {}
        }
    }
    // A newer look may finish first; an older one still lands when nothing newer has.
    if core.workspaces.applied.fetch_max(asked, Ordering::SeqCst) >= asked {
        return;
    }
    {
        let mut logged = core.workspaces.missed.locked();
        for (id, reason) in &missed {
            if logged.get(id) != Some(reason) {
                eprintln!("sikemux core: {id} cannot be started here: {reason}");
            }
        }
        *logged = missed;
    }
    core.workspaces.set_launchers(found);
    // The accounts' kept plan limits are listed by launcher, so they show once launchers are.
    core.changes.changed(Topic::Usage, None, None);
}

impl Workspaces {
    /// Each agent on each of the person's accounts, in the mode they start agents in:
    /// one launcher per profile, or one for an agent with none.
    pub(super) fn offered(&self) -> Vec<Launcher> {
        let published = self.published.locked();
        let saved = &published.saved;
        let profiles = saved
            .accounts
            .as_ref()
            .map(|book| book.profiles.as_slice())
            .unwrap_or_default();
        let mut offered = Vec::new();
        for def in AGENTS.iter() {
            let permission_mode = match saved.permission_mode.as_deref() {
                Some(mode) if def.bypass => mode,
                None if def.bypass => PermissionMode::Bypass.as_str(),
                _ => PermissionMode::WorkspaceWrite.as_str(),
            }
            .to_owned();
            let launcher = |id: String, label: String, agent: AgentSpec| Launcher {
                id,
                label,
                agent,
                permission_mode: permission_mode.clone(),
                status: None,
                program: None,
            };
            let own: Vec<_> = profiles
                .iter()
                .filter(|profile| profile.provider == def.kind)
                .collect();
            if own.is_empty() {
                let agent = AgentSpec::new(def.kind);
                offered.push(launcher(def.kind.into(), def.label.into(), agent));
            }
            for profile in &own {
                let label = match own.len() {
                    1 => def.label.to_owned(),
                    _ => format!("{} · {}", def.label, profile.name),
                };
                let id = format!("{}:{}", def.kind, profile.id);
                offered.push(launcher(id, label, profile.spec()));
            }
        }
        offered.truncate(MAX_LAUNCHERS);
        offered
    }

    pub(super) fn set_launchers(&self, launchers: Vec<Launcher>) {
        let (chats, terminals) = launchers
            .into_iter()
            .partition(|launcher| is_chat_provider(&launcher.agent.provider));
        let mut published = self.published.locked();
        (published.launchers, published.terminals) = (chats, terminals);
        drop(published);
        self.wake.now();
    }

    /// The launcher `id` names when its agent runs only in its own terminal.
    pub(crate) fn terminal_launcher(&self, id: &str) -> Option<Launcher> {
        self.published
            .locked()
            .terminals
            .iter()
            .find(|launcher| launcher.id == id)
            .cloned()
    }

    /// Keeps what the core last heard of an account's sign-in on the
    /// launchers that start on it.
    pub(crate) fn note_status(
        &self,
        provider: &str,
        config_path: Option<&str>,
        status: &AgentStatus,
    ) {
        let mut published = self.published.locked();
        let on_account = published.launchers.iter_mut().filter(|launcher| {
            launcher.agent.provider == provider
                && launcher.agent.config_path.as_deref() == config_path
        });
        for launcher in on_account {
            launcher.status = Some(status.clone());
        }
    }

    /// What `agent` starts with to run on `provider`, its keys read from the
    /// Keychain. Blocks.
    pub(crate) fn wiring(
        &self,
        agent: &str,
        provider: Option<&str>,
        model: Option<&str>,
    ) -> Result<Wiring, String> {
        self.providers
            .as_ref()
            .ok_or("Sikemux keeps no provider keys here")?
            .wiring(agent, provider, model)
    }

    /// What helpers run on, as the person set it in the app.
    pub(crate) fn helper(&self) -> Option<Helper> {
        self.providers.as_ref().and_then(Providers::helper)
    }

    /// A chat of `launcher` running `program` in `cwd`, built afresh, for an
    /// agent not yet named, on `provider` when one is named, and on `model`
    /// or the provider's coding model. Reads the providers' keys from the
    /// Keychain, so it blocks.
    pub(crate) fn chat_launch(
        &self,
        launcher: &Launcher,
        program: PathBuf,
        cwd: &Path,
        provider: Option<&str>,
        model: Option<String>,
    ) -> Result<ChatLaunch, String> {
        let agent = &launcher.agent;
        let model = model.or_else(|| {
            self.providers
                .as_ref()?
                .coding_model(provider?, &agent.provider)
        });
        let providers = match &self.providers {
            Some(providers) => providers.wiring(&agent.provider, provider, model.as_deref())?,
            None if provider.is_some() => return Err("Sikemux keeps no provider keys here".into()),
            None => Wiring::default(),
        };
        let model = match (self.providers.as_ref(), provider, model) {
            (Some(providers), Some(id), Some(model)) => Some(match providers.find(id) {
                Some(provider) => provider.agent_model(&agent.provider, &model),
                None => model,
            }),
            (_, _, model) => model,
        };
        let skills = self.data_dir.as_deref().and_then(skills::ready);
        let built = agent.chat(program, providers, skills);
        Ok(ChatLaunch {
            agent_id: String::new(),
            provider: agent.provider.clone(),
            cwd: cwd.to_path_buf(),
            program: built.program,
            args: built.args,
            env: built.env,
            mcp_servers: Vec::new(),
            resume_id: None,
            permission_mode: PermissionMode::parse(&launcher.permission_mode)?,
            model,
            model_provider: provider.map(str::to_owned),
            effort: None,
            account: None,
            fallbacks: Vec::new(),
        })
    }

    /// The launcher `id` names. An id with no profile, as a chat saved before
    /// its agent had accounts asks for, means the account the person chose. One
    /// the last look did not find is still returned, since starting it looks for
    /// its program again and says why it failed; one whose profile is gone falls
    /// back to its agent's own.
    pub(crate) fn launcher(&self, id: &str) -> CoreResult<Launcher> {
        let provider = id.split(':').next().unwrap_or(id);
        let wanted = {
            let published = self.published.locked();
            let chosen = (provider == id)
                .then(|| published.saved.accounts.as_ref())
                .flatten()
                .map(|book| format!("{provider}:{}", book.chosen(provider)));
            let wanted: Vec<String> = std::iter::once(id.to_owned()).chain(chosen).collect();
            let found = wanted.iter().find_map(|want| {
                published
                    .launchers
                    .iter()
                    .find(|launcher| &launcher.id == want)
                    .cloned()
            });
            if let Some(found) = found {
                return Ok(found);
            }
            wanted
        };
        let offered = self.offered();
        wanted
            .iter()
            .find_map(|want| offered.iter().find(|launcher| &launcher.id == want))
            .or_else(|| {
                offered
                    .iter()
                    .find(|launcher| launcher.agent.provider == provider)
            })
            .cloned()
            .ok_or_else(|| CoreError::from(format!("Sikemux has no agent called {provider}")))
    }

    /// Keeps the models and effort levels a launcher's session offered, so a chat of it that
    /// has not started, or a device choosing before one starts, offers the same.
    pub(crate) fn note_config_options(&self, launcher: &str, options: &Value) {
        if !options.is_array() {
            return;
        }
        let kept = self.published.locked().saved.launcher_options.get(launcher) == Some(options);
        if kept {
            return;
        }
        let _ = self.change(|saved| {
            saved
                .launcher_options
                .insert(launcher.to_owned(), options.clone());
            Ok(())
        });
    }

    /// Whether `launcher`'s agent is yet to be asked what it offers since the core started,
    /// which this call counts as asking.
    pub(crate) fn first_offer_read(&self, launcher: &str) -> bool {
        self.offers_read.locked().insert(launcher.to_owned())
    }

    /// The `configOptions` the launcher's last session offered, once one has.
    pub(crate) fn launcher_options(&self, launcher: &str) -> Option<Value> {
        self.published
            .locked()
            .saved
            .launcher_options
            .get(launcher)
            .cloned()
    }

    pub(crate) fn accounts(&self) -> Option<Book> {
        self.published.locked().saved.accounts.clone()
    }

    pub(crate) fn set_accounts(&self, book: Book) -> CoreResult<()> {
        book.check()?;
        self.change(|saved| {
            saved.accounts = Some(book);
            Ok(())
        })?;
        self.recent_stale();
        Ok(())
    }

    /// The agent `provider` on the person's account in `config_path`.
    pub(crate) fn account_spec(&self, provider: &str, config_path: Option<&str>) -> AgentSpec {
        let published = self.published.locked();
        match &published.saved.accounts {
            Some(book) => book.spec(provider, config_path),
            None => Book::default().spec(provider, config_path),
        }
    }

    /// The hosted model providers' keys `provider` reads. Reads the Keychain, so it blocks.
    pub(crate) fn provider_keys(&self, provider: &str) -> BTreeMap<String, String> {
        self.providers
            .as_ref()
            .map(|providers| providers.environment(provider))
            .unwrap_or_default()
    }

    /// Each offered account whose agent reports how much of its plan is used.
    pub(crate) fn usage_accounts(&self) -> Vec<UsageAccount> {
        self.published
            .locked()
            .launchers
            .iter()
            .filter(|launcher| {
                crate::agent_catalog::agent(&launcher.agent.provider)
                    .is_some_and(|agent| agent.usage)
            })
            .map(|launcher| UsageAccount {
                launcher: launcher.id.clone(),
                label: launcher.label.clone(),
                provider: launcher.agent.provider.clone(),
                config_path: launcher.agent.config_path.clone(),
                signed_in: matches!(launcher.status, Some(AgentStatus::Ready { .. })),
            })
            .collect()
    }

    /// The model providers the host's agents run on, and their keys.
    pub(crate) fn providers(&self) -> Option<&Providers> {
        self.providers.as_ref()
    }
}
