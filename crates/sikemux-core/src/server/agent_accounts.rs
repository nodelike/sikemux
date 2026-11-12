//! The core's calls about the person's agent accounts, which the app's
//! settings and rail make.

use crate::server::changed::ChangedCore;
use crate::server::changed::What;
use crate::server::parts::Parts;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, Once};

use serde::{Deserialize, Serialize};

use super::changed::Topic;
use super::connection::blocking;
use super::workspace::refresh_launchers;
use super::{Core, CoreResult};
use crate::agent_accounts::allocate::{self, Aim, Candidate, Placement};
use crate::agent_accounts::{chat_account, defaults, folders, status, usage, Book, Profile};
use crate::agent_catalog::{AgentEntry, AGENTS};
use crate::flights::{shared, Flights};
use crate::launch::account_environment;
use crate::launch::discovery;
use sikemux_process::sync::Locked;
use sikemux_wire::protocol::ChatAccount;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub provider: String,
    pub config_path: Option<String>,
}

/// Which account's sign-in to look at afresh; every account's without a provider.
#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Forget {
    pub provider: Option<String>,
    pub config_path: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatAccountsParams {
    pub agent_id: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveChatParams {
    pub agent_id: String,
    pub account_id: String,
}

/// The account a chat runs on, those it may move to, and why there are none when there are none,
/// as a client offers them.
#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct ChatAccounts {
    pub account: Option<NamedAccount>,
    pub others: Vec<NamedAccount>,
    pub none_because: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct NamedAccount {
    pub id: String,
    pub label: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Authenticate {
    pub agent_id: String,
    pub method_id: String,
}

#[derive(Serialize, Deserialize)]
pub struct Saving {
    pub profile: Profile,
}

#[derive(Serialize, Deserialize)]
pub struct Removing {
    pub id: String,
}

#[derive(Serialize, Deserialize)]
pub struct Choosing {
    pub provider: String,
    pub id: String,
}

/// A chat a pick moved, and the account it was on.
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MovedChat {
    pub agent_id: String,
    pub from: String,
}

/// A chat a pick left where it was, and why.
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct StayedChat {
    pub agent_id: String,
    pub why: String,
}

/// The accounts after a pick, what a client hands back to undo it, and the chats that stayed.
#[derive(Serialize, Deserialize)]
pub struct Picked {
    pub book: Book,
    pub undo: Unpicking,
    #[serde(default)]
    pub stayed: Vec<StayedChat>,
}

/// A pick to take back: the account picked, the one chosen before it, and the chats it moved.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Unpicking {
    pub provider: String,
    pub picked: String,
    pub back_to: String,
    /// The account new chats were pinned to before, when the pick pinned them.
    #[serde(default)]
    pub pinned_before: Option<String>,
    pub moved: Vec<MovedChat>,
}

/// Whether new chats go to the account whose week ends soonest, for every agent.
#[derive(Serialize, Deserialize)]
pub struct Smart {
    pub on: bool,
}

/// Whether new chats in the project at `project` may start on the account `account_id`.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InProject {
    pub account_id: String,
    pub project: String,
    pub allowed: bool,
}

/// The agent whose pin to take away.
#[derive(Serialize, Deserialize)]
pub struct Unpinning {
    pub provider: String,
}

#[derive(Serialize, Deserialize)]
pub struct Adding {
    pub provider: String,
    pub name: String,
}

/// The program the account's agent runs as, found afresh.
async fn program(core: &Core, account: &Account) -> CoreResult<PathBuf> {
    let spec = core
        .workspaces()
        .account_spec(&account.provider, account.config_path.as_deref());
    Ok(spec.program().await?)
}

/// Changes the person's accounts, offers agents on them afresh and tells
/// every client, answering with the accounts as they are now.
async fn change(
    core: &Arc<Core>,
    edit: impl FnOnce(&mut Book) -> Result<(), String>,
) -> CoreResult<Book> {
    let mut book = core.workspaces().accounts().unwrap_or_default();
    edit(&mut book)?;
    core.workspaces().set_accounts(book.clone())?;
    refresh_launchers(core.clone()).await;
    core.changed(What::Read(Topic::AgentAccounts));
    Ok(book.with_built_ins())
}

/// Moves `provider`'s chats onto the account `id` the person chose, where it
/// keeps their chats too, with a turn at work carrying on there. Agents in a
/// terminal stay where they are.
async fn move_chats(core: &Core, provider: &str, id: &str) -> (Vec<MovedChat>, Vec<StayedChat>) {
    let (mut moved, mut stayed) = (Vec::new(), Vec::new());
    for agent_id in core.workspaces().agent_ids() {
        let Some(agent) = core.workspaces().agent(&agent_id) else {
            continue;
        };
        let on = agent.profile_id.clone().unwrap_or_default();
        if agent.provider != provider || agent.terminal || on == id {
            continue;
        }
        let takers = match super::chat::accounts(core, &agent_id) {
            Ok(takers) => takers,
            Err(error) => {
                stayed.push(StayedChat {
                    agent_id,
                    why: error.to_string(),
                });
                continue;
            }
        };
        let Some(account) = takers.others.into_iter().find(|other| other.id == id) else {
            let why = takers
                .none_because
                .unwrap_or_else(|| "That account keeps its chats apart from this one".into());
            stayed.push(StayedChat { agent_id, why });
            continue;
        };
        match super::chat::move_chat(core, &agent_id, account).await {
            Ok(()) => moved.push(MovedChat { agent_id, from: on }),
            Err(error) => stayed.push(StayedChat {
                agent_id,
                why: error.to_string(),
            }),
        }
    }
    (moved, stayed)
}

/// One pick of an agent's account at a time, so that quick picks record each default and move
/// the chats in the order they ran.
fn pick_turn(provider: &str) -> Arc<tokio::sync::Mutex<()>> {
    static TURNS: LazyLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
        LazyLock::new(Mutex::default);
    TURNS
        .locked()
        .entry(provider.to_owned())
        .or_default()
        .clone()
}

pub(super) async fn set(core: &Arc<Core>, book: Book) -> CoreResult<()> {
    core.workspaces().set_accounts(book)?;
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
    Ok(pick(core, choosing).await?.book)
}

/// Makes the account the one the agent's chats start on. While smart is on, that pins new chats
/// to it and leaves open chats where they are; otherwise the open chats move there too.
pub(super) async fn pick(core: &Arc<Core>, choosing: Choosing) -> CoreResult<Picked> {
    let turn = pick_turn(&choosing.provider);
    let _turn = turn.lock().await;
    let before = core.workspaces().accounts().unwrap_or_default();
    let back_to = before.chosen(&choosing.provider);
    let pinned_before = before.pinned.get(&choosing.provider).cloned();
    let (provider, id) = (choosing.provider.clone(), choosing.id.clone());
    let book = change(core, move |book| {
        if book.smart {
            book.pinned.insert(provider.clone(), id.clone());
        }
        book.selected.insert(provider, id);
        Ok(())
    })
    .await?;
    let (moved, stayed) = match book.smart {
        true => (Vec::new(), Vec::new()),
        false => move_chats(core, &choosing.provider, &choosing.id).await,
    };
    Ok(Picked {
        book,
        undo: Unpicking {
            provider: choosing.provider,
            picked: choosing.id,
            back_to,
            pinned_before,
            moved,
        },
        stayed,
    })
}

/// Puts each chat a pick moved back on the account it was on, unless it has since gone to a third
/// account. A chat still on its way to the pick is sent back too, as its move is queued.
pub(super) async fn unpick(core: &Arc<Core>, undoing: Unpicking) -> CoreResult<Book> {
    let turn = pick_turn(&undoing.provider);
    let _turn = turn.lock().await;
    let (provider, back_to) = (undoing.provider.clone(), undoing.back_to.clone());
    let book = change(core, move |book| {
        let known = (book.clone().with_built_ins().profile(&back_to))
            .is_some_and(|profile| profile.provider == provider);
        if !known {
            return Err(format!("{back_to} is not an account of {provider}"));
        }
        match &undoing.pinned_before {
            Some(pinned) => book.pinned.insert(provider.clone(), pinned.clone()),
            None => book.pinned.remove(&provider),
        };
        book.selected.insert(provider, back_to);
        Ok(())
    })
    .await?;
    for moved in &undoing.moved {
        let Some(agent) = core.workspaces().agent(&moved.agent_id) else {
            continue;
        };
        let on = agent.profile_id.as_deref();
        if agent.provider != undoing.provider
            || on.is_some_and(|on| on != undoing.picked && on != moved.from)
        {
            continue;
        }
        let Some(target) = book
            .profile(&moved.from)
            .filter(|profile| profile.provider == undoing.provider)
        else {
            continue;
        };
        let account = chat_account(target);
        if let Err(error) = super::chat::move_chat(core, &moved.agent_id, account).await {
            eprintln!(
                "sikemux core: {} stayed on its account: {error}",
                moved.agent_id
            );
        }
    }
    Ok(book)
}

pub(super) async fn set_smart(core: &Arc<Core>, smart: Smart) -> CoreResult<Book> {
    change(core, move |book| {
        book.smart = smart.on;
        Ok(())
    })
    .await
}

/// Lets new chats in a project start on an account, or keeps them off it. A project then lists
/// the accounts it uses, and goes back to taking any once it lists them all.
pub(super) async fn set_in_project(core: &Arc<Core>, setting: InProject) -> CoreResult<Book> {
    change(core, move |book| {
        let every = book.clone().with_built_ins().profiles;
        let provider = (every.iter())
            .find(|profile| profile.id == setting.account_id)
            .map(|profile| profile.provider.clone())
            .ok_or("that account is not one Sikemux keeps")?;
        let ids = (book.projects.entry(setting.project.clone()))
            .or_insert_with(|| every.iter().map(|profile| profile.id.clone()).collect());
        ids.retain(|id| id != &setting.account_id);
        if setting.allowed {
            ids.push(setting.account_id.clone());
        }
        let keeps_one = (every.iter())
            .filter(|profile| profile.provider == provider)
            .any(|profile| ids.contains(&profile.id));
        if !keeps_one {
            return Err("A project keeps at least one account of each agent".into());
        }
        if every.iter().all(|profile| ids.contains(&profile.id)) {
            book.projects.remove(&setting.project);
        }
        Ok(())
    })
    .await
}

pub(super) async fn unpin(core: &Arc<Core>, unpinning: Unpinning) -> CoreResult<Book> {
    change(core, move |book| {
        book.pinned.remove(&unpinning.provider);
        Ok(())
    })
    .await
}

/// The person's accounts with `provider`'s agent that could take a new chat now: signed in,
/// each with its last reading. Empty while smart is off or fewer than two could, so one account
/// costs nothing.
fn candidates(book: &Book, provider: &str) -> Vec<(Profile, Option<usage::Reading>)> {
    if !book.smart {
        return Vec::new();
    }
    let signed_in = signed_in(book, provider);
    if signed_in.len() < 2 {
        return Vec::new();
    }
    (signed_in.into_iter())
        .map(|profile| {
            let reading = usage::last_reading(provider, profile.config_path.as_deref());
            (profile, reading)
        })
        .collect()
}

/// The person's accounts with `provider`'s agent that are signed in.
fn signed_in(book: &Book, provider: &str) -> Vec<Profile> {
    (book.clone().with_built_ins().profiles.into_iter())
        .filter(|profile| profile.provider == provider)
        .filter(|profile| {
            let env = account_environment(provider, profile.config_path.as_deref())
                .into_iter()
                .collect();
            matches!(
                crate::agent_accounts::sign_in::signed_in(provider, &env),
                Some(crate::agent_accounts::sign_in::SignIn::Account(_))
            )
        })
        .collect()
}

/// Another chat agent the person is signed in to with room for a chat in `cwd`, for a chat
/// whose own agent has no usage left on any account it may use. An account not read yet counts
/// as having room.
pub(crate) fn other_agent_with_room(
    book: &Book,
    provider: &str,
    cwd: &Path,
) -> Option<&'static str> {
    let now = sikemux_process::clock::epoch_ms();
    let has_room = |agent: &AgentEntry| {
        let readings: Vec<(Profile, Option<usage::Reading>)> = (signed_in(book, agent.kind)
            .into_iter())
        .filter(|profile| book.allowed_in(cwd, &profile.id))
        .map(|profile| {
            let reading = usage::last_reading(agent.kind, profile.config_path.as_deref());
            (profile, reading)
        })
        .collect();
        let offered: Vec<Candidate> = (readings.iter())
            .map(|(profile, reading)| Candidate {
                id: &profile.id,
                usage: reading.as_ref().map(|reading| &reading.usage),
                typical: None,
                burn: usage::burn(agent.kind, profile.config_path.as_deref()),
            })
            .collect();
        offered.iter().any(|candidate| candidate.usage.is_none())
            || allocate::place(&offered, None, Aim::Headroom, now).is_some()
    };
    AGENTS
        .iter()
        .filter(|agent| agent.kind != provider && agent.usage && agent.chat.is_some())
        .find(|agent| has_room(agent))
        .map(|agent| agent.kind)
}

/// The account a chat of `provider` on `model` goes to while smart is on, and why: the one the
/// person pinned for a new chat, else the one [`allocate::place`] picks. None while smart is
/// off, the agent has one account here, or no reading leaves any room.
pub(crate) fn placement(
    book: &Book,
    provider: &str,
    chat: &Placing,
    aim: Aim,
    among: impl Fn(&Profile) -> bool,
) -> Option<Placement> {
    let allowed = |profile: &Profile| {
        (chat.cwd.as_deref()).is_none_or(|cwd| book.allowed_in(cwd, &profile.id))
    };
    let candidates: Vec<_> = (candidates(book, provider).into_iter())
        .filter(|(profile, _)| among(profile) && allowed(profile))
        .collect();
    let pinned = book.pinned.get(provider).filter(|_| aim == Aim::Soonest);
    if let Some(pinned) = pinned.filter(|id| candidates.iter().any(|(p, _)| &p.id == *id)) {
        return Some(Placement {
            id: pinned.clone(),
            why: "You pinned new chats to it".into(),
        });
    }
    let offered: Vec<Candidate> = (candidates.iter())
        .map(|(profile, reading)| {
            let usage = reading.as_ref().map(|reading| &reading.usage);
            let weight = usage.map_or(1.0, |usage| {
                allocate::weight(&usage.provider, usage.plan.as_deref())
            });
            Candidate {
                id: &profile.id,
                usage,
                typical: chat
                    .typical_tokens
                    .zip(percent_per_token(&profile.id, weight))
                    .map(|(tokens, percent)| tokens as f64 * percent),
                burn: usage::burn(provider, profile.config_path.as_deref()),
            }
        })
        .collect();
    let model = chat.model.as_deref();
    let placed = allocate::place(&offered, model, aim, sikemux_process::clock::epoch_ms());
    let chosen = book.chosen(provider);
    let chosen_allowed = (book.clone().with_built_ins().profile(&chosen)).is_some_and(allowed);
    placed.or_else(|| {
        let first = candidates.first().filter(|_| !chosen_allowed)?;
        Some(Placement {
            id: first.0.id.clone(),
            why: "It is an account this project uses".into(),
        })
    })
}

/// What is known of the chat being placed: its model, and the tokens a typical chat in its
/// project spends.
#[derive(Default)]
pub(crate) struct Placing {
    pub model: Option<String>,
    pub typical_tokens: Option<u64>,
    /// Where it works, for the accounts its project uses.
    pub cwd: Option<PathBuf>,
}

/// What its chats' tokens cost one account's week, from how much they spent while it filled.
struct Cost {
    at: u64,
    used: f64,
    reset: Option<u64>,
    weight: f64,
    percent: f64,
    tokens: f64,
}

fn costs() -> std::sync::MutexGuard<'static, HashMap<String, Cost>> {
    static COSTS: LazyLock<Mutex<HashMap<String, Cost>>> = LazyLock::new(Mutex::default);
    COSTS.locked()
}

/// The share of the account's week one token takes: as its own chats showed, else as another
/// account's did, scaled by how many small plans each holds.
fn percent_per_token(account: &str, weight: f64) -> Option<f64> {
    let costs = costs();
    let rate = |cost: &Cost| (cost.tokens > 0.0).then(|| cost.percent / cost.tokens);
    if let Some(own) = costs.get(account).and_then(rate) {
        return Some(own);
    }
    costs
        .values()
        .find_map(|cost| Some(rate(cost)? * cost.weight / weight))
}

/// Counts what the account's chats spent since its week last rose, once it rose by a point.
fn note_cost(core: &Core, account: &str, reading: &usage::Usage) {
    let now = sikemux_process::clock::epoch_ms();
    let Some(week) = reading.week() else {
        return;
    };
    let (used, reset) = (
        week.used_at(now),
        week.resets_at.as_ref().and_then(|at| at.unix_ms()),
    );
    let weight = allocate::weight(&reading.provider, reading.plan.as_deref());
    let since = {
        let mut costs = costs();
        let cost = costs.entry(account.to_owned()).or_insert(Cost {
            at: now,
            used,
            reset,
            weight,
            percent: 0.0,
            tokens: 0.0,
        });
        if cost.reset != reset {
            (cost.at, cost.used, cost.reset) = (now, used, reset);
            return;
        }
        let risen = used - cost.used;
        if risen < 1.0 {
            return;
        }
        let since = cost.at;
        (cost.at, cost.used, cost.weight) = (now, used, weight);
        (since, risen)
    };
    let (log, account) = (core.activity_log().clone(), account.to_owned());
    tokio::task::spawn_blocking(move || {
        let tokens = log.tokens_on(&account, since.0, now);
        if tokens == 0 {
            return;
        }
        if let Some(cost) = costs().get_mut(&account) {
            cost.percent += since.1;
            cost.tokens += tokens as f64;
        }
    });
}

/// How long a chat waits to start for its accounts' readings to come in, before it is placed
/// on what is already known.
const FRESH_READINGS_WITHIN: std::time::Duration = std::time::Duration::from_secs(3);

/// Reads again the plan of each of `provider`'s accounts whose reading is older than a
/// lookup is kept, waiting a moment for them; a slow lookup carries on behind.
pub(crate) async fn freshen(core: Arc<Core>, book: Book, provider: String) {
    let provider = provider.as_str();
    let now = sikemux_process::clock::epoch_ms();
    let old = |reading: &Option<usage::Reading>| {
        reading.as_ref().is_none_or(|reading| {
            now.saturating_sub(reading.read_at) > usage::USAGE_TTL.as_millis() as u64
        })
    };
    let lookups: Vec<_> = (candidates(&book, provider).into_iter())
        .filter(|(_, reading)| old(reading))
        .map(|(profile, _)| {
            let (core, provider) = (core.clone(), provider.to_owned());
            tokio::spawn(async move {
                let account = Account {
                    provider: provider.clone(),
                    config_path: profile.config_path.clone(),
                };
                if let Ok(program) = program(&core, &account).await {
                    let config_path = profile.config_path.as_deref();
                    let _ = usage::usage(&provider, &program, config_path).await;
                    core.changed(What::Read(Topic::Usage));
                }
            })
        })
        .collect();
    if lookups.is_empty() {
        return;
    }
    let _ = tokio::time::timeout(
        FRESH_READINGS_WITHIN,
        futures_util::future::join_all(lookups),
    )
    .await;
}

pub(super) async fn add(adding: Adding) -> CoreResult<String> {
    blocking(move || Ok(folders::add(&adding.provider, &adding.name)?)).await
}

/// Where an account's agent program is, what was tried and failed on the
/// way, and whether it is signed in. The program comes from the person's own
/// profile, never from the caller.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Availability {
    pub program: Option<PathBuf>,
    pub failures: Vec<String>,
    pub status: status::AgentStatus,
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
            blocking(move || Ok(core.workspaces().provider_keys(&provider)))
                .await
                .unwrap_or_default()
        };
        let config_path = config_path.as_deref();
        let checked = status::ask(&provider, &program, config_path, keys).await;
        if status::keep(&provider, config_path, &checked, generation) {
            core.workspaces()
                .note_status(&provider, config_path, &checked.status);
            core.changed(What::View);
            core.changed(What::Read(Topic::AgentStatus));
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
                core.changed(What::Read(Topic::AgentStatus));
            }
        });
    });
}

pub(super) async fn availability(core: &Arc<Core>, account: Account) -> CoreResult<Availability> {
    follow_late_answers(core);
    let (provider, config_path) = (&account.provider, account.config_path.as_deref());
    let spec = core.workspaces().account_spec(provider, config_path);
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
    if !crate::agent_catalog::agent(&account.provider).is_some_and(|agent| agent.accounts) {
        return Err(crate::agent_accounts::NO_ACCOUNTS.into());
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

/// The account whose limit reset to spend, by the launcher its usage row names.
#[derive(Serialize, Deserialize)]
pub struct UsingReset {
    pub launcher: String,
    /// The reset to spend, by its kind's id; none spends the one the agent would.
    #[serde(default)]
    pub kind: Option<String>,
}

/// What spending a reset did, and the resets the account holds after it.
#[derive(Serialize, Deserialize)]
pub struct UsedReset {
    pub outcome: usage::ResetOutcome,
    pub left: Option<u32>,
}

/// Spends one of the account's limit resets and reads its limits again, telling clients.
pub(super) async fn use_reset(core: &Arc<Core>, using: UsingReset) -> CoreResult<UsedReset> {
    let Some(found) = (core.workspaces().usage_accounts().into_iter())
        .find(|account| account.launcher == using.launcher)
    else {
        return Err("That account is no longer offered".into());
    };
    let account = Account {
        provider: found.provider,
        config_path: found.config_path,
    };
    let program = program(core, &account).await?;
    let config_path = account.config_path.as_deref();
    let spend = crate::agent_catalog::AgentKind::from_name(&account.provider)
        .and_then(|agent| {
            agent
                .ops()
                .use_reset(&program, config_path, using.kind.as_deref())
        })
        .ok_or("This agent offers no limit resets")?;
    let outcome = spend.await?;
    usage::forget_lookup(&account.provider, config_path);
    let left = usage::usage(&account.provider, &program, config_path)
        .await
        .ok()
        .and_then(|read| read.resets)
        .map(|resets| resets.left);
    core.changed(What::Read(Topic::Usage));
    Ok(UsedReset { outcome, left })
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
    let book = core.workspaces().accounts().unwrap_or_default();
    let config_path = account
        .and_then(|id| book.profile(id))
        .and_then(|profile| profile.config_path.clone());
    signed_out(core, provider, config_path.as_deref());
}

fn signed_out(core: &Core, provider: &str, config_path: Option<&str>) {
    let signed_out = status::AgentStatus::SignedOut;
    core.workspaces()
        .note_status(provider, config_path, &signed_out);
    status::remember(provider, config_path, signed_out);
    core.changed(What::View);
}

pub(super) fn retry_failed(core: &Core, failed: ChatAccountsParams) -> CoreResult<()> {
    core.chats().running(&failed.agent_id)?.retry_failed()
}

pub(super) fn discard_failed(core: &Core, failed: ChatAccountsParams) -> CoreResult<()> {
    core.chats().running(&failed.agent_id)?.discard_failed()
}

pub(super) fn chat_accounts(core: &Core, chat: ChatAccountsParams) -> CoreResult<ChatAccounts> {
    let named = |account: ChatAccount| NamedAccount {
        id: account.id,
        label: account.label,
    };
    let takers = super::chat::accounts(core, &chat.agent_id)?;
    Ok(ChatAccounts {
        account: takers.current.map(named),
        others: takers.others.into_iter().map(named).collect(),
        none_because: takers.none_because,
    })
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
    /// Whether smart accounts would start the agent's next new chat on it.
    #[serde(default)]
    pub next: bool,
    /// Whether the person pinned the agent's new chats to it.
    #[serde(default)]
    pub pinned: bool,
}

/// How old a reading may grow before it says so: three of the app's reads missed.
const READING_STALE_AFTER_MS: u64 = 15 * 60 * 1000;

impl AccountUsage {
    /// Whether its numbers are old news at `now_ms`: the last lookup failed, or the reading
    /// is well past the next one due.
    pub fn is_stale(&self, now_ms: u64) -> bool {
        self.problem.is_some()
            || self
                .read_at
                .is_some_and(|read| now_ms.saturating_sub(read) >= READING_STALE_AFTER_MS)
    }
}

/// Keeps the plan limits a chat's agent reported on the account `account_id` as that account's
/// reading, telling clients when it changed.
pub(crate) fn note_live(
    core: &Core,
    provider: &str,
    account_id: Option<&str>,
    live: Vec<usage::UsageWindow>,
) {
    let book = core
        .workspaces()
        .accounts()
        .unwrap_or_default()
        .with_built_ins();
    let profile = account_id.and_then(|id| book.profile(id));
    let config_path = profile.and_then(|profile| profile.config_path.as_deref());
    if usage::note_live(provider, config_path, live) {
        core.changed(What::Read(Topic::Usage));
    }
    let reading = usage::last_reading(provider, config_path);
    if let (Some(account), Some(reading)) = (account_id, reading) {
        note_cost(core, account, &reading.usage);
    }
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
    core.changed(What::Read(Topic::Usage));
    found
}

async fn usages_read(core: &Arc<Core>, ask: bool) -> Vec<AccountUsage> {
    let mut found = Vec::new();
    for account in core.workspaces().usage_accounts() {
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
                note_sign_in(core, &account.provider, config_path, &lookup);
                lookup.err()
            }
        };
        let reading = usage::last_reading(&account.provider, config_path);
        let only_when_read =
            !crate::agent_catalog::agent(&account.provider).is_some_and(|agent| agent.accounts);
        let unread = reading.is_none()
            || problem.as_deref() == Some(crate::agent_accounts::plan_limits::NO_LIMITS);
        if only_when_read && unread {
            continue;
        }
        found.push(AccountUsage {
            launcher: account.launcher,
            provider: account.provider,
            label: account.label,
            signed_in: account.signed_in,
            read_at: reading.as_ref().map(|reading| reading.read_at),
            usage: reading.map(|reading| reading.usage),
            problem,
            next: false,
            pinned: false,
        });
    }
    mark_next(core, &mut found);
    found
}

/// Tells the launchers what a plan lookup showed of the account's sign-in, which nothing else
/// asks for an agent the person added no accounts to.
fn note_sign_in(
    core: &Arc<Core>,
    provider: &str,
    config_path: Option<&str>,
    lookup: &Result<usage::Usage, String>,
) {
    let was_out = status::cached(provider, config_path) == Some(status::AgentStatus::SignedOut);
    match lookup {
        Err(error) if error == usage::SIGNED_OUT && !was_out => {
            signed_out(core, provider, config_path)
        }
        Ok(_) if was_out => {
            status::forget(Some(provider), config_path);
            tokio::spawn(refresh_launchers(core.clone()));
        }
        _ => {}
    }
}

/// Marks the account each agent's next new chat goes to, and the one the person pinned.
fn mark_next(core: &Core, found: &mut [AccountUsage]) {
    let book = core.workspaces().accounts().unwrap_or_default();
    let providers: std::collections::BTreeSet<String> = found
        .iter()
        .map(|account| account.provider.clone())
        .collect();
    for provider in providers {
        let next = placement(&book, &provider, &Placing::default(), Aim::Soonest, |_| {
            true
        });
        let pinned = book.pinned.get(&provider).filter(|_| book.smart);
        for account in found
            .iter_mut()
            .filter(|account| account.provider == provider)
        {
            let id = super::workspace::LauncherId::parse(&account.launcher).account;
            account.next = next
                .as_ref()
                .is_some_and(|next| Some(next.id.as_str()) == id);
            account.pinned = pinned.is_some_and(|pinned| Some(pinned.as_str()) == id);
        }
    }
}

/// An agent Sikemux can run, as this host has it on the account the person chose for it.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Installed {
    #[serde(rename = "type")]
    pub kind: String,
    pub label: String,
    pub command: String,
    pub available: bool,
    pub error: Option<String>,
    /// The programs passed over on the way to the one found.
    pub warning: Option<String>,
    pub profile_id: Option<String>,
    pub config_path: Option<String>,
    pub default_model: Option<String>,
    pub default_effort: Option<String>,
    pub status: status::AgentStatus,
    /// The release of its CLI installed here, when the CLI names one.
    #[serde(default)]
    pub version: Option<String>,
    /// The releases its chats run now, newest first.
    #[serde(default)]
    pub running: Vec<String>,
    /// The newest release out.
    #[serde(default)]
    pub latest: Option<String>,
    /// How an update the person asked for goes.
    #[serde(default)]
    pub updating: Option<super::agent_updates::Updating>,
    /// The models updating to the newest release lets it run, by name.
    #[serde(default)]
    pub unlocks: Vec<String>,
}

/// The agents found on this host, each on the account chosen for it. An agent that
/// keeps accounts is listed even when it is not found, so its account can be set up.
pub(super) async fn installed(core: &Arc<Core>) -> CoreResult<Vec<Installed>> {
    let book = core.workspaces().accounts().unwrap_or_default();
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
            kind: def.kind.to_owned(),
            label: def.label.to_owned(),
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
            version: None,
            running: Vec::new(),
            latest: None,
            updating: None,
            unlocks: Vec::new(),
        })
    }))
    .await
}

/// Every agent the person can set up here: those found, and those Sikemux can install
/// that it did not find.
pub(super) async fn setup(core: &Arc<Core>) -> CoreResult<Vec<Installed>> {
    let mut agents = setup_list(installed(core).await?);
    super::agent_updates::add_versions(core, &mut agents);
    Ok(agents)
}

/// `found` with the installable agents it lacks, ready ones first, then the featured,
/// each in the catalog's order.
fn setup_list(mut agents: Vec<Installed>) -> Vec<Installed> {
    let missing: Vec<&AgentEntry> = AGENTS
        .iter()
        .filter(|def| def.install.is_some() && !agents.iter().any(|agent| agent.kind == def.kind))
        .collect();
    agents.extend(missing.into_iter().map(|def| Installed {
        kind: def.kind.to_owned(),
        label: def.label.to_owned(),
        command: def.command.to_owned(),
        available: false,
        error: None,
        warning: None,
        profile_id: None,
        config_path: None,
        default_model: None,
        default_effort: None,
        status: status::AgentStatus::Missing,
        version: None,
        running: Vec::new(),
        latest: None,
        updating: None,
        unlocks: Vec::new(),
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
            kind: kind.to_owned(),
            label: kind.to_owned(),
            command: kind.to_owned(),
            available,
            error: None,
            warning: None,
            profile_id: None,
            config_path: None,
            default_model: None,
            default_effort: None,
            status,
            version: None,
            running: Vec::new(),
            latest: None,
            updating: None,
            unlocks: Vec::new(),
        }
    }

    #[test]
    fn setup_adds_every_agent_it_can_install_but_did_not_find() {
        let ready = status::AgentStatus::Ready { account: None };
        let all = setup_list(vec![agent("claude", ready.clone(), true)]);
        let installable = AGENTS.iter().filter(|def| def.install.is_some());
        for def in installable {
            let listed = all
                .iter()
                .filter(|agent| agent.kind == def.kind)
                .collect::<Vec<_>>();
            assert_eq!(listed.len(), 1, "{}", def.kind);
            if def.kind == "claude" {
                assert_eq!(listed[0].status, ready);
            } else {
                assert!(!listed[0].available, "{}", def.kind);
                assert_eq!(listed[0].status, status::AgentStatus::Missing);
            }
        }
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
            .map(|agent| agent.kind.as_str())
            .filter(|kind| given.contains(kind))
            .collect();
        assert_eq!(kinds, ["opencode", "claude", "codex", "hermes", "grok"]);
    }
}
