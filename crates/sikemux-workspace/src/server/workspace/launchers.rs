//! What the host offers to start agents with: each launcher, the accounts and model providers
//! behind it, and the launch a chat gets from them.

use super::*;
use crate::agent_accounts::{by_hand, BUILT_IN};
use crate::server::changed::ChangedCore;
use crate::server::changed::What;
use crate::server::workspace_state::WorkspaceParts;
use sikemux_view::workspace::{ConfigChoice, ConfigPicker};

/// A launcher's id: the CLI it starts and the account it starts it on, written `kind` or
/// `kind:account`.
#[derive(Clone, Copy)]
pub struct LauncherId<'a> {
    pub kind: &'a str,
    pub account: Option<&'a str>,
}

impl<'a> LauncherId<'a> {
    pub fn parse(id: &'a str) -> Self {
        match id.split_once(':') {
            Some((kind, account)) => Self {
                kind,
                account: Some(account),
            },
            None => Self {
                kind: id,
                account: None,
            },
        }
    }

    pub fn on(kind: &'a str, account: &'a str) -> Self {
        Self {
            kind,
            account: Some(account),
        }
    }
}

impl std::fmt::Display for LauncherId<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.account {
            Some(account) => write!(f, "{}:{account}", self.kind),
            None => f.write_str(self.kind),
        }
    }
}

/// The launcher that starts `agent`: its CLI on the account it runs on.
pub fn launcher_of(agent: &WorkspaceAgent) -> String {
    LauncherId {
        kind: &agent.provider,
        account: agent.profile_id.as_deref(),
    }
    .to_string()
}

/// The launcher, of those offered as (id, agent kind), that starts agents of `kind`: the one on the
/// account the person chose for them, else the first of that kind, or the only one of a kind
/// whose account the person alone picks.
pub fn launcher_for<'a>(
    offered: impl Iterator<Item = (&'a str, &'a str)> + Clone,
    accounts: Option<&Book>,
    kind: &str,
) -> Option<String> {
    let chosen = accounts
        .and_then(|book| match by_hand(kind) {
            true => Some(book.chosen(kind)),
            false => book.selected.get(kind).cloned(),
        })
        .map(|account| LauncherId::on(kind, &account).to_string());
    let mut of_kind = offered.filter(|(_, provider)| *provider == kind);
    chosen
        .filter(|id| of_kind.clone().any(|(offered, _)| offered == id))
        .or_else(|| {
            of_kind
                .find(|(id, _)| !by_hand(kind) || *id == kind)
                .map(|(id, _)| id.to_owned())
        })
}

/// The launcher that found `chat`. A chat that left the recent ones keeps no launcher, so
/// it takes the account the person chose for its agent.
pub fn chat_launcher<'a>(published: &'a Published, chat: &Recent) -> Option<&'a Launcher> {
    let chosen = published
        .saved
        .accounts
        .as_ref()
        .map(|book| LauncherId::on(&chat.provider, &book.chosen(&chat.provider)).to_string());
    published
        .every_launcher()
        .filter(|launcher| launcher.agent.provider == chat.provider)
        .find(|launcher| {
            launcher.id == chat.launcher
                || (chat.launcher.is_empty() && Some(&launcher.id) == chosen.as_ref())
        })
        .or_else(|| {
            chat.launcher.is_empty().then(|| {
                published.every_launcher().find(|launcher| {
                    launcher.agent.provider == chat.provider
                        && (!by_hand(&chat.provider) || launcher.id == chat.provider)
                })
            })?
        })
}

/// How to start `chat` again on the launcher it last ran on, or the account the person
/// chose for its agent when it keeps none.
pub fn choice_of(
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
        terminal: false,
    })
}

pub struct ChatChoice {
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
    /// It runs in its CLI in a terminal, even for an agent that can chat.
    pub terminal: bool,
}

/// A launcher as devices see it, with the models and effort its last session offered.
pub fn launcher_info(saved: &Saved, launcher: &Launcher) -> LauncherInfo {
    let config_options = offered(&saved.launcher_options, &launcher.id)
        .cloned()
        .unwrap_or(Value::Null);
    let (model, effort) = pickers::pickers(&config_options);
    let effort = effort.or_else(|| catalog_efforts(&launcher.agent.provider));
    LauncherInfo {
        id: launcher.id.clone(),
        provider: launcher.agent.provider.clone(),
        label: launcher.label.clone(),
        permission_mode: launcher.permission_mode,
        config_options,
        status: launcher
            .status
            .as_ref()
            .map(|status| status.state().to_owned()),
        model,
        effort,
        recent_models: (saved.recent_models.get(&launcher.agent.provider))
            .cloned()
            .unwrap_or_default(),
    }
}

/// What `launcher`'s agent offered last, or while it has said nothing on its account, what it
/// offered on another.
fn offered<'a>(kept: &'a BTreeMap<String, Value>, launcher: &str) -> Option<&'a Value> {
    let kind = LauncherId::parse(launcher).kind;
    let of_a_profile = format!("{kind}:");
    let others = kept
        .iter()
        .filter(|(id, _)| id.starts_with(&of_a_profile))
        .map(|(_, options)| options);
    (kept.get(launcher).into_iter())
        .chain(kept.get(kind))
        .chain(others)
        .find(|options| !says_nothing(options))
}

fn says_nothing(options: &Value) -> bool {
    options.as_array().is_none_or(Vec::is_empty)
}

/// How many models a kind of agent's recent list keeps.
const MOST_RECENT_MODELS: usize = 5;

/// The efforts `provider`'s CLI takes, for a launcher no session has said them for yet.
fn catalog_efforts(provider: &str) -> Option<ConfigPicker> {
    let agent = crate::agent_catalog::agent(provider)?;
    let offered = agent.chat.as_ref().is_some_and(|chat| chat.offers_effort());
    (offered && !agent.efforts.is_empty()).then(|| ConfigPicker {
        id: "effort".to_owned(),
        current: String::new(),
        choices: agent
            .efforts
            .iter()
            .map(|effort| ConfigChoice {
                value: (*effort).to_owned(),
                label: pickers::level_name(effort),
                description: None,
                is_default: false,
                superseded: false,
                maker: None,
            })
            .collect(),
    })
}

/// An account the agent picker offers whose agent reports its plan usage.
pub struct UsageAccount {
    pub launcher: String,
    pub label: String,
    pub provider: String,
    pub config_path: Option<String>,
    pub signed_in: bool,
}

/// Finds each offered agent's program and offers those found, with what
/// the core last heard of their sign-in. Runs as the core starts and when the
/// person's accounts or their mode change, or they may have installed an agent.
pub fn refresh_launchers(core: Arc<Core>) -> impl std::future::Future<Output = ()> {
    let asked = core.workspaces().asked.fetch_add(1, Ordering::SeqCst) + 1;
    async move { refresh(core, asked).await }
}

/// Answers once the core has looked for the agents it can start at least once, so one asked
/// to start an agent just after it started doesn't take "not looked yet" for "none".
pub async fn launchers_known(core: &Arc<Core>) {
    if core.workspaces().applied.load(Ordering::SeqCst) == 0 {
        refresh_launchers(core.clone()).await;
    }
}

pub async fn refresh(core: Arc<Core>, asked: u64) {
    let looking: Vec<_> = core
        .workspaces()
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
    if core.workspaces().applied.fetch_max(asked, Ordering::SeqCst) >= asked {
        return;
    }
    {
        let mut logged = core.workspaces().missed.locked();
        for (id, reason) in &missed {
            if logged.get(id) != Some(reason) {
                eprintln!("sikemux core: {id} cannot be started here: {reason}");
            }
        }
        *logged = missed;
    }
    core.workspaces().set_launchers(found);
    // The accounts' kept plan limits are listed by launcher, so they show once launchers are.
    core.changed(What::Read(Topic::Usage));
}

impl Workspaces {
    /// Each agent on each of the person's accounts, in the mode they start agents in:
    /// one launcher per profile, or one for an agent with none.
    pub fn offered(&self) -> Vec<Launcher> {
        let published = self.published.locked();
        let saved = &published.saved;
        let kept = saved
            .accounts
            .as_ref()
            .map(|book| book.profiles.as_slice())
            .unwrap_or_default();
        let every = saved.accounts.clone().map(|book| book.with_built_ins());
        let every = every
            .as_ref()
            .map(|book| book.profiles.as_slice())
            .unwrap_or_default();
        let mut offered = Vec::new();
        for def in AGENTS.iter() {
            let permission_mode = match saved.permission_mode.unwrap_or(PermissionMode::Bypass) {
                PermissionMode::Bypass if def.bypass => PermissionMode::Bypass,
                PermissionMode::Auto if def.auto => PermissionMode::Auto,
                _ => PermissionMode::WorkspaceWrite,
            };
            let launcher = |id: String, label: String, agent: AgentSpec| Launcher {
                id,
                label,
                agent,
                permission_mode,
                status: None,
                program: None,
            };
            let kept_own = kept.iter().any(|profile| profile.provider == def.kind);
            let own: Vec<_> = (every.iter())
                .filter(|profile| kept_own && profile.provider == def.kind)
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
                let id = LauncherId::on(def.kind, &profile.id).to_string();
                offered.push(launcher(id, label, profile.spec()));
            }
        }
        offered.truncate(MAX_LAUNCHERS);
        offered
    }

    pub fn set_launchers(&self, launchers: Vec<Launcher>) {
        let (chats, terminals) = launchers
            .into_iter()
            .partition(|launcher| is_chat_provider(&launcher.agent.provider));
        let mut published = self.published.locked();
        (published.launchers, published.terminals) = (chats, terminals);
        drop(published);
        self.wake.now();
    }

    /// The launcher `id` names when its agent runs only in its own terminal.
    pub fn terminal_launcher(&self, id: &str) -> Option<Launcher> {
        self.published
            .locked()
            .terminals
            .iter()
            .find(|launcher| launcher.id == id)
            .cloned()
    }

    /// Keeps what the core last heard of an account's sign-in on the
    /// launchers that start on it.
    pub fn note_status(&self, provider: &str, config_path: Option<&str>, status: &AgentStatus) {
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
    pub fn wiring(
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
    pub fn helper(&self) -> Option<Helper> {
        self.providers.as_ref().and_then(Providers::helper)
    }

    /// A chat of `launcher` running `program` in `cwd`, built afresh, for an
    /// agent not yet named, on `provider` when one is named, and on `model`
    /// or the provider's coding model. Reads the providers' keys from the
    /// Keychain, so it blocks.
    pub fn chat_launch(
        &self,
        launcher: &Launcher,
        program: PathBuf,
        cwd: &Path,
        provider: Option<&str>,
        model: Option<String>,
    ) -> Result<ChatLaunch, String> {
        let agent = &launcher.agent;
        let routed = match (provider, &model, &self.providers) {
            (None, Some(model), Some(providers)) => providers.routing(model),
            _ => None,
        };
        let provider = provider.or(routed.as_ref().map(|routed| routed.id.as_str()));
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
            permission_mode: launcher.permission_mode,
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
    /// back to its agent's own, unless the person alone picks that agent's accounts.
    pub fn launcher(&self, id: &str) -> CoreResult<Launcher> {
        let provider = LauncherId::parse(id).kind;
        let wanted = {
            let published = self.published.locked();
            let chosen = (provider == id)
                .then(|| published.saved.accounts.as_ref())
                .flatten()
                .map(|book| LauncherId::on(provider, &book.chosen(provider)).to_string());
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
                offered.iter().find(|launcher| {
                    launcher.agent.provider == provider
                        && (!by_hand(provider)
                            || launcher.id == provider
                                && wanted.iter().all(|want| {
                                    (LauncherId::parse(want).account)
                                        .is_none_or(|account| account.starts_with(BUILT_IN))
                                }))
                })
            })
            .cloned()
            .ok_or_else(|| {
                let known = by_hand(provider)
                    .then(|| crate::agent_catalog::agent(provider))
                    .flatten();
                CoreError::from(match known {
                    Some(agent) => format!(
                        "That {} account is not on this computer any more. Pick another for this chat.",
                        agent.label
                    ),
                    None => format!("Sikemux has no agent called {provider}"),
                })
            })
    }

    /// Keeps the models and effort levels a launcher's session offered, so a chat of it that
    /// has not started, or a device choosing before one starts, offers the same.
    pub fn note_config_options(&self, launcher: &str, options: &Value) {
        if !options.is_array() {
            return;
        }
        let mut options = options.clone();
        let kind = LauncherId::parse(launcher).kind;
        let published = self.published.locked();
        if let Some(before) = offered(&published.saved.launcher_options, launcher) {
            pickers::carry_efforts(&mut options, before);
        }
        if let Some(pick) = published.saved.model_picks.get(kind) {
            pick.mark(&mut options);
        }
        let kept = published.saved.launcher_options.get(launcher) == Some(&options);
        drop(published);
        if kept {
            return;
        }
        let _ = self.change(|saved| {
            saved.launcher_options.insert(launcher.to_owned(), options);
            Ok(())
        });
    }

    /// Puts the model a new chat starts on first in its agent's recent list. Claude and Codex
    /// offer a handful of models, so they keep none.
    pub fn note_started_model(&self, launch: &ChatLaunch) {
        let listed = !matches!(
            AgentKind::from_name(&launch.provider),
            None | Some(AgentKind::Claude | AgentKind::Codex)
        );
        let Some(model) = launch
            .model
            .clone()
            .filter(|_| listed && launch.resume_id.is_none())
        else {
            return;
        };
        let used = RecentModel {
            model,
            provider: launch.model_provider.clone(),
        };
        let first = (self
            .published
            .locked()
            .saved
            .recent_models
            .get(&launch.provider))
        .and_then(|recent| recent.first())
            == Some(&used);
        if first {
            return;
        }
        let _ = self.change(|saved| {
            let recent = saved
                .recent_models
                .entry(launch.provider.clone())
                .or_default();
            recent.retain(|kept| *kept != used);
            recent.insert(0, used);
            recent.truncate(MOST_RECENT_MODELS);
            Ok(())
        });
    }

    /// The model and effort the person last picked for `kind`.
    /// The models `kind`'s chats last started on, newest first.
    pub fn recent_models(&self, kind: &str) -> Vec<RecentModel> {
        let saved = &self.published.locked().saved;
        saved.recent_models.get(kind).cloned().unwrap_or_default()
    }

    pub fn model_pick(&self, kind: &str) -> Option<ModelPick> {
        self.published.locked().saved.model_picks.get(kind).cloned()
    }

    /// Keeps a model or effort the person picked for `kind`, so its new chats start on it.
    pub fn keep_model_pick(&self, kind: &str, pick: ModelPick) -> CoreResult<()> {
        self.change(|saved| {
            let kept = saved.model_picks.entry(kind.to_owned()).or_default();
            kept.model = pick.model.or(kept.model.take());
            kept.effort = pick.effort.or(kept.effort.take());
            let kept = kept.clone();
            let of_a_profile = format!("{kind}:");
            for (launcher, options) in saved.launcher_options.iter_mut() {
                if launcher == kind || launcher.starts_with(&of_a_profile) {
                    kept.mark(options);
                }
            }
            Ok(())
        })
    }

    /// Whether `launcher`'s agent can be asked what it offers now, which this call counts as
    /// asking until [`Self::offer_read`]: not while another ask of it is still out.
    pub fn start_offer_read(&self, launcher: &str) -> bool {
        self.offers_asking.locked().insert(launcher.to_owned())
    }

    /// Ends the ask of `launcher`'s agent, which went as `read` says.
    pub fn offer_read(&self, launcher: &str, read: OfferRead) {
        self.offers_read.locked().insert(launcher.to_owned(), read);
        self.offers_asking.locked().remove(launcher);
    }

    /// Forgets that the last ask of `launcher`'s agent failed, as it is asked again; one signed
    /// out stays so until the ask says otherwise. Whether there was one to forget.
    pub fn forget_failed_read(&self, launcher: &str) -> bool {
        let mut reads = self.offers_read.locked();
        let failed = reads
            .get(launcher)
            .and_then(|read| read.unread.as_ref())
            .is_some_and(|unread| unread["kind"] != "signIn");
        if failed {
            reads.remove(launcher);
        }
        failed
    }

    /// How the last ask of `launcher`'s agent since the core started went.
    pub fn last_offer_read(&self, launcher: &str) -> Option<OfferRead> {
        self.offers_read.locked().get(launcher).cloned()
    }

    /// The `configOptions` the launcher's agent offered last, or while none are kept for its
    /// account, another account's.
    pub fn launcher_options(&self, launcher: &str) -> Option<Value> {
        offered(&self.published.locked().saved.launcher_options, launcher).cloned()
    }

    /// Whether what the launcher's agent offers is kept for its own account.
    pub fn keeps_options(&self, launcher: &str) -> bool {
        let published = self.published.locked();
        (published.saved.launcher_options.get(launcher))
            .is_some_and(|options| !says_nothing(options))
    }

    pub fn accounts(&self) -> Option<Book> {
        self.published.locked().saved.accounts.clone()
    }

    pub fn set_accounts(&self, book: Book) -> CoreResult<()> {
        book.check()?;
        self.change(|saved| {
            saved.accounts = Some(book);
            Ok(())
        })?;
        self.recent_stale();
        Ok(())
    }

    /// The agent `provider` on the person's account in `config_path`.
    pub fn account_spec(&self, provider: &str, config_path: Option<&str>) -> AgentSpec {
        let published = self.published.locked();
        match &published.saved.accounts {
            Some(book) => book.spec(provider, config_path),
            None => Book::default().spec(provider, config_path),
        }
    }

    /// Tells `agent` of the connected providers it reads, adding to `env` and `args`. Blocks.
    pub fn list_connected(
        &self,
        agent: &str,
        env: &mut BTreeMap<String, String>,
        args: &mut Vec<String>,
    ) -> Result<(), String> {
        match &self.providers {
            Some(providers) => providers.list_connected(agent, env, args),
            None => Ok(()),
        }
    }

    /// The hosted model providers' keys `provider` reads, for telling whether it can sign in.
    /// Reads the Keychain, so it blocks.
    pub fn provider_keys(&self, provider: &str) -> BTreeMap<String, String> {
        self.providers
            .as_ref()
            .map(|providers| providers.environment(provider))
            .unwrap_or_default()
    }

    /// The agents a chat can be started on here, one launcher for each account.
    pub fn chat_launchers(&self) -> Vec<Launcher> {
        self.published.locked().launchers.clone()
    }

    /// Each offered account whose agent reports how much of its plan is used.
    pub fn usage_accounts(&self) -> Vec<UsageAccount> {
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
    pub fn providers(&self) -> Option<&Providers> {
        self.providers.as_ref()
    }
}
