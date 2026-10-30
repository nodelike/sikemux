//! The core's calls about the person's agent accounts, which the app's
//! settings and rail make.

use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Once};

use serde::{Deserialize, Serialize};

use super::changed::Topic;
use super::connection::blocking;
use super::workspace::refresh_launchers;
use super::{Core, CoreResult};
use crate::agent_accounts::{chat_account, defaults, folders, status, usage, Book, Profile};
use crate::agent_catalog::{AgentEntry, AGENTS};
use crate::flights::{shared, Flights};
use crate::launch::discovery;
use crate::protocol::ChatAccount;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Account {
    provider: String,
    config_path: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Forget {
    provider: Option<String>,
    config_path: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Resuming {
    agent_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Moving {
    agent_id: String,
    account_id: String,
}

/// The account a chat runs on and those it may move to, as a client offers them.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatAccounts {
    account: Option<Named>,
    others: Vec<Named>,
}

#[derive(Serialize)]
pub(super) struct Named {
    id: String,
    label: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Authenticate {
    agent_id: String,
    method_id: String,
}

#[derive(Deserialize)]
pub(super) struct Saving {
    profile: Profile,
}

#[derive(Deserialize)]
pub(super) struct Removing {
    id: String,
}

#[derive(Deserialize)]
pub(super) struct Choosing {
    provider: String,
    id: String,
}

#[derive(Deserialize)]
pub(super) struct Switching {
    provider: String,
    on: bool,
}

#[derive(Deserialize)]
pub(super) struct Adding {
    provider: String,
    name: String,
}

/// The program the account's agent runs as, found afresh.
async fn program(core: &Core, account: &Account) -> CoreResult<PathBuf> {
    let spec = core
        .workspaces
        .account_spec(&account.provider, account.config_path.as_deref());
    Ok(spec.program().await?)
}

/// Changes the person's accounts, offers agents on them afresh and tells
/// every client, answering with the accounts as they are now.
async fn change(
    core: &Arc<Core>,
    edit: impl FnOnce(&mut Book) -> Result<(), String>,
) -> CoreResult<Book> {
    let mut book = core.workspaces.accounts().unwrap_or_default();
    edit(&mut book)?;
    core.workspaces.set_accounts(book.clone())?;
    refresh_launchers(core.clone()).await;
    core.changes.changed(Topic::AgentAccounts, None, None);
    Ok(book)
}

/// Moves `provider`'s chats onto the account `id` the person chose, where it
/// keeps their chats too, with a turn at work carrying on there. Agents in a
/// terminal stay where they are.
fn move_chats(core: &Core, book: &Book, provider: &str, id: &str) {
    let Some(target) = book
        .profile(id)
        .filter(|profile| profile.provider == provider)
    else {
        return;
    };
    let chosen = book.chosen(provider);
    for agent_id in core.workspaces.agent_ids() {
        let Some(agent) = core.workspaces.agent(&agent_id) else {
            continue;
        };
        let on = agent.profile_id.clone().unwrap_or_else(|| chosen.clone());
        if agent.provider != provider || agent.pty_id.is_some() || on == id {
            continue;
        }
        let from = book
            .profile(&on)
            .and_then(|profile| profile.config_path.as_deref());
        if !folders::sessions_shared(provider, from, target.config_path.as_deref()) {
            continue;
        }
        let account = chat_account(
            provider,
            &target.id,
            &target.name,
            target.config_path.as_deref(),
        );
        if let Err(error) = super::chat::switch_account(core, &agent_id, account) {
            eprintln!("sikemux core: {agent_id} stayed on its account: {error}");
        }
    }
}

pub(super) async fn set(core: &Arc<Core>, book: Book) -> CoreResult<()> {
    core.workspaces.set_accounts(book)?;
    refresh_launchers(core.clone()).await;
    Ok(())
}

pub(super) async fn save(core: &Arc<Core>, saving: Saving) -> CoreResult<Book> {
    change(core, |book| {
        book.save(saving.profile);
        Ok(())
    })
    .await
}

pub(super) async fn remove(core: &Arc<Core>, removing: Removing) -> CoreResult<Book> {
    change(core, |book| book.remove(&removing.id)).await
}

pub(super) async fn select(core: &Arc<Core>, choosing: Choosing) -> CoreResult<Book> {
    let (provider, id) = (choosing.provider.clone(), choosing.id.clone());
    let book = change(core, move |book| {
        book.selected.insert(provider, id);
        Ok(())
    })
    .await?;
    move_chats(core, &book, &choosing.provider, &choosing.id);
    Ok(book)
}

pub(super) async fn set_auto_switch(core: &Arc<Core>, switching: Switching) -> CoreResult<Book> {
    change(core, move |book| {
        book.auto_switch.insert(switching.provider, switching.on);
        Ok(())
    })
    .await
}

pub(super) async fn add(adding: Adding) -> CoreResult<String> {
    blocking(move || Ok(folders::add(&adding.provider, &adding.name)?)).await
}

/// Where an account's agent program is, what was tried and failed on the
/// way, and whether it is signed in. The program comes from the person's own
/// profile, never from the caller.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Availability {
    program: Option<PathBuf>,
    failures: Vec<String>,
    status: status::AgentStatus,
}

/// Asks an account again, once however many ask together, and tells every
/// client when the answer changed.
async fn recheck(
    core: Arc<Core>,
    provider: String,
    config_path: Option<String>,
    program: PathBuf,
) -> status::Checked {
    static CHECKING: LazyLock<Flights<String, status::Checked>> = LazyLock::new(Flights::default);
    let generation = status::generation();
    let flight = format!(
        "{generation}\0{provider}\0{}",
        config_path.as_deref().unwrap_or("")
    );
    shared(&CHECKING, flight, async {
        let keys = {
            let (core, provider) = (core.clone(), provider.clone());
            blocking(move || Ok(core.workspaces.provider_keys(&provider)))
                .await
                .unwrap_or_default()
        };
        let config_path = config_path.as_deref();
        let checked = status::ask(&provider, &program, config_path, keys).await;
        if status::keep(&provider, config_path, &checked, generation) {
            core.workspaces
                .note_status(&provider, config_path, &checked.status);
            core.view_changed();
            core.changes.changed(Topic::AgentStatus, None, None);
        }
        checked
    })
    .await
}

/// What is known of an account, at once whenever anything is: an answer that
/// no longer holds is given while the account is asked again behind it.
async fn checked(
    core: &Arc<Core>,
    provider: &str,
    config_path: Option<&str>,
    program: &Path,
) -> status::Checked {
    let ask = || {
        recheck(
            core.clone(),
            provider.to_owned(),
            config_path.map(str::to_owned),
            program.to_owned(),
        )
    };
    match status::known(provider, config_path) {
        Some((checked, true)) => checked,
        Some((checked, false)) => {
            tokio::spawn(ask());
            checked
        }
        None => ask().await,
    }
}

/// Offers agents afresh and tells every client each time a program whose
/// check timed out answers in the background.
fn follow_late_answers(core: &Arc<Core>) {
    static FOLLOWING: Once = Once::new();
    FOLLOWING.call_once(|| {
        let core = core.clone();
        tokio::spawn(async move {
            loop {
                discovery::answered_late().notified().await;
                refresh_launchers(core.clone()).await;
                core.changes.changed(Topic::AgentStatus, None, None);
            }
        });
    });
}

pub(super) async fn availability(core: &Arc<Core>, account: Account) -> CoreResult<Availability> {
    follow_late_answers(core);
    let (provider, config_path) = (&account.provider, account.config_path.as_deref());
    let spec = core.workspaces.account_spec(provider, config_path);
    let (program, failures) = status::locate(&spec).await;
    let status = match &program {
        Some(program) => checked(core, provider, config_path, program).await.status,
        None => status::unfound(&failures),
    };
    Ok(Availability {
        program,
        failures,
        status,
    })
}

pub(super) async fn status(core: &Arc<Core>, account: Account) -> CoreResult<status::AgentStatus> {
    Ok(availability(core, account).await?.status)
}

pub(super) async fn account_status(
    core: &Arc<Core>,
    account: Account,
) -> CoreResult<status::AccountStatus> {
    if !matches!(account.provider.as_str(), "claude" | "codex") {
        return Err("Only Claude and Codex keep separate accounts".into());
    }
    let program = program(core, &account).await?;
    let (provider, config_path) = (&account.provider, account.config_path.as_deref());
    let mut known = checked(core, provider, config_path, &program).await;
    if known.account.is_none() {
        known = recheck(core.clone(), provider.clone(), account.config_path, program).await;
    }
    Ok(known
        .account
        .ok_or("Claude or Codex did not say who is signed in")??)
}

pub(super) async fn usage(core: &Arc<Core>, account: Account) -> CoreResult<usage::Usage> {
    let program = program(core, &account).await?;
    let config_path = account.config_path.as_deref();
    Ok(usage::usage(&account.provider, &program, config_path).await?)
}

pub(super) fn forget(core: &Arc<Core>, forget: Forget) {
    let (provider, config_path) = (forget.provider.as_deref(), forget.config_path.as_deref());
    status::forget(provider, config_path);
    if let Some(provider) = provider {
        usage::forget(provider, config_path);
    }
    discovery::forget_failures();
    tokio::spawn(refresh_launchers(core.clone()));
}

pub(super) fn mark_signed_out(core: &Core, account: Account) {
    signed_out(core, &account.provider, account.config_path.as_deref());
}

/// A chat on `account`, one of the person's profiles or the default one when absent, was
/// refused for sign-in: the account is signed out now, whatever its CLI last said.
pub(crate) fn chat_refused(core: &Core, provider: &str, account: Option<&str>) {
    let book = core.workspaces.accounts().unwrap_or_default();
    let config_path = account
        .and_then(|id| book.profile(id))
        .and_then(|profile| profile.config_path.clone());
    signed_out(core, provider, config_path.as_deref());
}

fn signed_out(core: &Core, provider: &str, config_path: Option<&str>) {
    let signed_out = status::AgentStatus::SignedOut;
    core.workspaces
        .note_status(provider, config_path, &signed_out);
    status::remember(provider, config_path, signed_out);
    core.view_changed();
}

pub(super) fn cancel_resume(core: &Arc<Core>, resuming: Resuming) {
    super::chat::reset::cancel_by_person(core, &resuming.agent_id);
}

fn failed_chat(core: &Core, failed: &Resuming) -> CoreResult<Arc<super::chat::Chat>> {
    Ok(core
        .chats
        .get(&failed.agent_id)
        .ok_or_else(|| format!("no chat {} is running", failed.agent_id))?)
}

pub(super) fn retry_failed(core: &Core, failed: Resuming) -> CoreResult<()> {
    failed_chat(core, &failed)?.retry_failed()
}

pub(super) fn discard_failed(core: &Core, failed: Resuming) -> CoreResult<()> {
    failed_chat(core, &failed)?.discard_failed()
}

pub(super) fn chat_accounts(core: &Core, chat: Resuming) -> CoreResult<ChatAccounts> {
    let named = |account: ChatAccount| Named {
        id: account.id,
        label: account.label,
    };
    let (account, others) = super::chat::accounts(core, &chat.agent_id)?;
    Ok(ChatAccounts {
        account: account.map(named),
        others: others.into_iter().map(named).collect(),
    })
}

pub(super) fn move_chat(core: &Core, moving: Moving) -> CoreResult<()> {
    super::chat::move_to_account(core, &moving.agent_id, &moving.account_id)
}

pub(super) async fn authenticate(core: &Arc<Core>, params: Authenticate) -> CoreResult<()> {
    super::chat::authenticate(core, &params.agent_id, params.method_id).await?;
    Ok(())
}

/// How much of its plan an account has used, as the core last read it.
#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AccountUsage {
    /// The launcher that starts agents on the account.
    pub launcher: String,
    pub provider: String,
    /// The account as the agent picker names it.
    pub label: String,
    pub signed_in: bool,
    /// Its plan and rolling windows as its CLI last reported them, however long ago.
    pub usage: Option<usage::Usage>,
    /// When that reading was taken, in Unix milliseconds.
    pub read_at: Option<u64>,
    /// Why the latest lookup found no reading.
    pub problem: Option<String>,
}

/// Every offered account's usage as last read, starting no agent's CLI.
pub(super) async fn usages(core: &Arc<Core>) -> Vec<AccountUsage> {
    usages_read(core, false).await
}

/// Reads every offered account's usage again from its CLI, as the person asked, and tells
/// clients. Each lookup asks the agent's service with the person's sign-in.
pub(super) async fn read_usages(core: &Arc<Core>) -> Vec<AccountUsage> {
    let found = usages_read(core, true).await;
    static FIRST: std::sync::Once = std::sync::Once::new();
    FIRST.call_once(|| {
        let read = found.iter().filter(|account| account.problem.is_none()).count();
        eprintln!(
            "sikemux core: read plan limits for {} accounts, {read} answered (first read since start)",
            found.len()
        );
    });
    core.changes.changed(Topic::Usage, None, None);
    found
}

async fn usages_read(core: &Arc<Core>, ask: bool) -> Vec<AccountUsage> {
    let mut found = Vec::new();
    for account in core.workspaces.usage_accounts() {
        let config_path = account.config_path.as_deref();
        let problem = match ask {
            false => usage::last_problem(&account.provider, config_path),
            true => {
                let asked = Account {
                    provider: account.provider.clone(),
                    config_path: account.config_path.clone(),
                };
                let lookup = match program(core, &asked).await {
                    Ok(program) => usage::usage(&account.provider, &program, config_path).await,
                    Err(error) => Err(error.to_string()),
                };
                lookup.err()
            }
        };
        let reading = usage::last_reading(&account.provider, config_path);
        found.push(AccountUsage {
            launcher: account.launcher,
            provider: account.provider,
            label: account.label,
            signed_in: account.signed_in,
            read_at: reading.as_ref().map(|reading| reading.read_at),
            usage: reading.map(|reading| reading.usage),
            problem,
        });
    }
    found
}

/// An agent Sikemux can run, as this host has it on the account the person chose for it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Installed {
    #[serde(rename = "type")]
    kind: &'static str,
    label: &'static str,
    command: String,
    available: bool,
    error: Option<String>,
    /// The programs passed over on the way to the one found.
    warning: Option<String>,
    profile_id: Option<String>,
    config_path: Option<String>,
    default_model: Option<String>,
    default_effort: Option<String>,
    status: status::AgentStatus,
}

/// The agents found on this host, each on the account chosen for it. An agent that
/// keeps accounts is listed even when it is not found, so its account can be set up.
pub(super) async fn installed(core: &Arc<Core>) -> CoreResult<Vec<Installed>> {
    let book = core.workspaces.accounts().unwrap_or_default();
    let listed = AGENTS.iter().filter_map(|def| {
        let chosen = book.chosen(def.kind);
        let accounts = || {
            book.profiles
                .iter()
                .filter(|profile| profile.provider == def.kind)
        };
        let profile = accounts()
            .find(|profile| profile.id == chosen)
            .or_else(|| accounts().next());
        let explicit = profile.and_then(|profile| profile.executable_path.as_deref());
        (def.accounts || !discovery::candidates(def, explicit).is_empty())
            .then_some((def, profile, explicit))
    });
    futures_util::future::try_join_all(listed.map(|(def, profile, explicit)| async move {
        let config_path = profile.and_then(|profile| profile.config_path.clone());
        let account = Account {
            provider: def.kind.to_owned(),
            config_path: config_path.clone(),
        };
        let Availability {
            program,
            failures,
            status,
        } = availability(core, account).await?;
        let warning = program.as_ref().and_then(|path| {
            (!failures.is_empty())
                .then(|| format!("Skipped {}; using {}", failures.join("; "), path.display()))
        });
        let error = program.is_none().then(|| match failures.is_empty() {
            true => format!("{} executable was not found", def.label),
            false => failures.join("; "),
        });
        Ok(Installed {
            kind: def.kind,
            label: def.label,
            command: program
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned())
                .or_else(|| explicit.map(str::to_owned))
                .unwrap_or_else(|| def.command.to_owned()),
            available: program.is_some(),
            error,
            warning,
            profile_id: profile.map(|profile| profile.id.clone()),
            default_model: defaults::configured_default_model(def.kind, config_path.as_deref()),
            default_effort: defaults::configured_default_effort(def.kind, config_path.as_deref()),
            config_path,
            status,
        })
    }))
    .await
}

/// Every agent the person can set up here: those found, and those Sikemux can install
/// that it did not find.
pub(super) async fn setup(core: &Arc<Core>) -> CoreResult<Vec<Installed>> {
    Ok(setup_list(installed(core).await?))
}

/// `found` with the installable agents it lacks, ready ones first, then the featured,
/// each in the catalog's order.
fn setup_list(mut agents: Vec<Installed>) -> Vec<Installed> {
    let missing: Vec<&AgentEntry> = AGENTS
        .iter()
        .filter(|def| def.install.is_some() && !agents.iter().any(|agent| agent.kind == def.kind))
        .collect();
    agents.extend(missing.into_iter().map(|def| Installed {
        kind: def.kind,
        label: def.label,
        command: def.command.to_owned(),
        available: false,
        error: None,
        warning: None,
        profile_id: None,
        config_path: None,
        default_model: None,
        default_effort: None,
        status: status::AgentStatus::Missing,
    }));
    agents.sort_by_key(|agent| {
        let ready = agent.available && agent.status != status::AgentStatus::SignedOut;
        let at = AGENTS.iter().position(|def| def.kind == agent.kind);
        let featured = at.is_some_and(|at| AGENTS[at].featured);
        (!ready, !featured, at)
    });
    agents
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(kind: &'static str, status: status::AgentStatus, available: bool) -> Installed {
        Installed {
            kind,
            label: kind,
            command: kind.to_owned(),
            available,
            error: None,
            warning: None,
            profile_id: None,
            config_path: None,
            default_model: None,
            default_effort: None,
            status,
        }
    }

    #[test]
    fn setup_adds_every_agent_it_can_install_but_did_not_find() {
        let ready = status::AgentStatus::Ready { account: None };
        let all = setup_list(vec![agent("claude", ready, true)]);
        let kinds: Vec<&str> = all.iter().map(|agent| agent.kind).collect();
        assert_eq!(
            kinds,
            ["claude", "codex", "hermes", "pi", "opencode", "omp", "grok"]
        );
        assert!(!all[1].available);
        assert_eq!(all[1].status, status::AgentStatus::Missing);
    }

    #[test]
    fn setup_puts_ready_agents_first_then_claude_and_codex_then_the_rest() {
        use status::AgentStatus::{Missing, Ready, SignedOut};
        let ordered = setup_list(vec![
            agent("grok", Missing, false),
            agent("codex", SignedOut, true),
            agent(
                "opencode",
                Ready {
                    account: Some("apiKey".into()),
                },
                true,
            ),
            agent("hermes", Missing, false),
            agent("claude", Missing, false),
        ]);
        let given = ["grok", "codex", "opencode", "hermes", "claude"];
        let kinds: Vec<&str> = ordered
            .iter()
            .map(|agent| agent.kind)
            .filter(|kind| given.contains(kind))
            .collect();
        assert_eq!(kinds, ["opencode", "claude", "codex", "hermes", "grok"]);
    }
}
