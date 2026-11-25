//! The call Sikemux puts to the person when an account's provider refuses its chats, for an
//! unpaid plan or a lapsed sign-in: one per account and project, naming the chats it stopped and
//! the accounts they could carry on with.

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::calls::{CallAsker, CallKind, CallOption, CallOutcome, CallResult, PersonCall};
use sikemux_view::chat::FailureKind as Why;

use super::agent_accounts::{stranded, NamedAccount};
use super::chat_state::ChatParts;
use super::person_calls::{self, Raise, ACCOUNT_REFUSED};
use super::workspace_state::WorkspaceParts;
use super::Core;
use crate::agent_accounts::allocate::{self, Candidate};
use crate::agent_accounts::{by_hand, usage};

const MOVE: &str = "move:";
const RETRY: &str = "retry";
const LEAVE: &str = "leave";
/// How long refusals gather before the call is put, so chats stopped together share one.
const GATHER: Duration = Duration::from_secs(3);

/// Whether any of these calls is open: unknown until the first look, then no or yes.
static OPEN: AtomicU8 = AtomicU8::new(UNKNOWN);
const UNKNOWN: u8 = 2;

/// The chats the person left stopped, by account, until their stop is over.
static LEFT: Mutex<BTreeSet<(String, String)>> = Mutex::new(BTreeSet::new());

/// A chat's turn ended, refused for its account or not: the calls are looked at again when that
/// could change them.
pub(crate) fn turn_ended(core: &Arc<Core>, refused: bool) {
    if refused || OPEN.load(Ordering::Acquire) != 0 {
        soon(core);
    }
}

/// Puts or updates the calls shortly, once the chats a refusal stopped have all reported it.
fn soon(core: &Arc<Core>) {
    static WAITING: AtomicBool = AtomicBool::new(false);
    if WAITING.swap(true, Ordering::AcqRel) {
        return;
    }
    let core = core.clone();
    tokio::spawn(async move {
        tokio::time::sleep(GATHER).await;
        WAITING.store(false, Ordering::Release);
        sync(&core);
    });
}

/// One open call's account, project and chats, as its key holds them.
struct Asked {
    id: String,
    project: String,
    account: String,
    chats: BTreeSet<String>,
}

fn key(account: &str, chats: &BTreeSet<String>) -> String {
    let chats: Vec<&str> = chats.iter().map(String::as_str).collect();
    format!("{ACCOUNT_REFUSED}{account}:{}", chats.join(","))
}

fn parse(key: &str) -> Option<(String, BTreeSet<String>)> {
    let (account, chats) = key.strip_prefix(ACCOUNT_REFUSED)?.split_once(':')?;
    let chats = chats.split(',').filter(|id| !id.is_empty());
    Some((account.to_owned(), chats.map(str::to_owned).collect()))
}

/// Raises a call for each account and project with chats newly stopped, and withdraws or redraws
/// the open ones whose chats moved on. A chat not running counts as still stopped, so a call
/// outlives a restart.
fn sync(core: &Core) {
    let mut wanted: HashMap<(String, String), (Why, NamedAccount, Vec<NamedAccount>)> =
        HashMap::new();
    let mut stopped: HashMap<(String, String), BTreeSet<String>> = HashMap::new();
    let held = stranded(core);
    let mut left = LEFT.locked();
    left.retain(|(account, chat)| {
        (held.iter()).any(|held| held.account.id == *account && held.chats.contains(chat))
    });
    for held in held {
        for chat in &held.chats {
            if left.contains(&(held.account.id.clone(), chat.clone())) {
                continue;
            }
            let project = core.names().get(chat).map(|named| named.project);
            let Some(project) = project.filter(|project| !project.is_empty()) else {
                continue;
            };
            let at = (project, held.account.id.clone());
            stopped.entry(at.clone()).or_default().insert(chat.clone());
            wanted
                .entry(at)
                .or_insert_with(|| (held.why, held.account.clone(), held.to.clone()));
        }
    }
    drop(left);
    let asked: Vec<Asked> = (person_calls::raised_open(core).into_iter())
        .filter_map(|call| {
            let (account, chats) = parse(&person_calls::key_of(core, &call.id)?)?;
            Some(Asked {
                id: call.id,
                project: call.project,
                account,
                chats,
            })
        })
        .collect();
    let mut kept = false;
    for call in asked {
        let at = (call.project.clone(), call.account.clone());
        let now = stopped.entry(at).or_default();
        let still = (call.chats.iter())
            .filter(|chat| core.chats().running(chat).is_err() && core.names().get(chat).is_some());
        now.extend(still.cloned());
        if *now != call.chats {
            person_calls::set_outcome(core, &call.id, ended(CallResult::Withdrawn));
        } else {
            now.clear();
            kept = true;
        }
    }
    let mut open = false;
    for (at, chats) in stopped {
        let Some((why, account, to)) = wanted.remove(&at) else {
            continue;
        };
        if chats.is_empty() {
            continue;
        }
        open = true;
        let raise = call_for(core, at.0, why, &account, &to, &chats);
        if let Err(problem) = person_calls::raise(core, raise) {
            eprintln!(
                "sikemux core: chats an account stopped were not put to the person: {problem}"
            );
        }
    }
    OPEN.store(u8::from(open || kept), Ordering::Release);
}

fn call_for(
    core: &Core,
    project: String,
    why: Why,
    account: &NamedAccount,
    to: &[NamedAccount],
    chats: &BTreeSet<String>,
) -> Raise {
    let names: Vec<String> = (chats.iter())
        .map(|chat| format!("@{}", core.names().name_or_id(chat)))
        .collect();
    let them = match names.as_slice() {
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
        [] => String::new(),
    };
    let count = match chats.len() {
        1 => "1 chat".to_owned(),
        many => format!("{many} chats"),
    };
    let label = &account.label;
    let resumes = (chats.iter()).all(|chat| core.chats().resets.resumes_at(chat).is_some());
    let (kind, question, context, retry) = match why {
        Why::SignIn => (
            CallKind::Access,
            format!("{label} is signed out, so {count} stopped"),
            format!("{them} stopped when the provider refused {label}'s sign-in."),
            Some(("Sign in again, then retry them", "From Settings, Accounts")),
        ),
        Why::Limit => (
            CallKind::Access,
            format!("{label} is out of usage, so {count} stopped"),
            format!("{them} stopped at {label}'s usage limit."),
            None,
        ),
        _ => (
            CallKind::Money,
            format!("{label} is past due, so {count} stopped"),
            format!("{them} stopped when the provider refused {label}'s unpaid plan."),
            Some(("Pay, then retry them", "At claude.ai/settings/billing")),
        ),
    };
    let book = core
        .workspaces()
        .accounts()
        .unwrap_or_default()
        .with_built_ins();
    let provider = book
        .profile(&account.id)
        .map(|profile| profile.provider.clone());
    let readings: Vec<(&NamedAccount, Option<usage::Reading>)> = (to.iter())
        .map(|other| {
            let profile = book.profile(&other.id);
            let config_path = profile.and_then(|profile| profile.config_path.as_deref());
            let reading = (provider.as_deref())
                .and_then(|provider| usage::last_reading(provider, config_path));
            (other, reading)
        })
        .collect();
    let candidates: Vec<Candidate> = (readings.iter())
        .map(|(other, reading)| Candidate {
            id: &other.id,
            usage: reading.as_ref().map(|reading| &reading.usage),
            burn: (provider.as_deref()).map_or(0.0, |provider| {
                let profile = book.profile(&other.id);
                usage::burn(
                    provider,
                    profile.and_then(|profile| profile.config_path.as_deref()),
                )
            }),
        })
        .collect();
    let by_hand = provider.as_deref().is_some_and(by_hand);
    let pick = match by_hand {
        true => (readings.iter())
            .min_by(|(_, one), (_, other)| {
                let used = |reading| nearest(reading).unwrap_or(f64::INFINITY);
                used(one.as_ref()).total_cmp(&used(other.as_ref()))
            })
            .map(|(other, _)| other.id.clone()),
        false => allocate::place(&candidates, allocate::Chat::default(), epoch_ms())
            .map(|placed| placed.id)
            .or_else(|| to.first().map(|other| other.id.clone())),
    };
    let mut moves: Vec<CallOption> = (readings.iter())
        .map(|(other, reading)| CallOption {
            id: format!("{MOVE}{}", other.id),
            label: match pick.as_deref() == Some(other.id.as_str()) {
                _ if why == Why::Limit && chats.len() == 1 => {
                    format!("Carry on with {}", other.label)
                }
                _ if why == Why::Limit => format!("Carry them on with {}", other.label),
                true => format!("Move all {} to {}", chats.len(), other.label),
                false => format!("Move them to {}", other.label),
            },
            detail: Some(match why {
                Why::Limit => room(reading.as_ref()),
                _ => used(reading.as_ref()),
            }),
            picture: None,
        })
        .collect();
    moves.sort_by_key(|option| option.id.strip_prefix(MOVE) != pick.as_deref());
    let option = |id: &str, (label, detail): (&str, &str)| CallOption {
        id: id.to_owned(),
        label: label.to_owned(),
        detail: (!detail.is_empty()).then(|| detail.to_owned()),
        picture: None,
    };
    moves.extend(retry.map(|retry| option(RETRY, retry)));
    let leave = match (why, resumes) {
        (Why::Limit, true) => ("Wait for the limit to reset", "They carry on by themselves"),
        _ => ("Leave them stopped", ""),
    };
    moves.push(option(LEAVE, leave));
    Raise {
        project,
        kind,
        asker: CallAsker {
            agent_id: String::new(),
            name: "Sikemux".to_owned(),
            cli: "sikemux".to_owned(),
            chief: false,
        },
        question,
        context: Some(context),
        options: moves,
        pick: pick.map(|id| format!("{MOVE}{id}")),
        against: None,
        evidence: Vec::new(),
        action: None,
        brief: None,
        place: None,
        key: Some(key(&account.id, chats)),
        yes: None,
    }
}

/// How full the account's shortest window is, such as "22% of its session used".
fn used(reading: Option<&usage::Reading>) -> String {
    let window = reading.and_then(|reading| {
        (reading.usage.windows.iter())
            .filter(|window| window.scope.is_none())
            .min_by_key(|window| window.window_minutes.unwrap_or(u64::MAX))
    });
    match window {
        Some(window) if window.window_minutes.is_some_and(|minutes| minutes <= 300) => {
            format!("{:.0}% of its session used", window.used_percent)
        }
        Some(window) => format!("{:.0}% of its {} used", window.used_percent, window.label),
        None => "Its use is not read yet".to_owned(),
    }
}

/// How much the account has left before its nearest limit, such as "62% left".
fn room(reading: Option<&usage::Reading>) -> String {
    match nearest(reading) {
        Some(used) => format!("{:.0}% left", (100.0 - used).max(0.0)),
        None => "Its use is not read yet".to_owned(),
    }
}

/// How much of the limit it is nearest to the account has used.
fn nearest(reading: Option<&usage::Reading>) -> Option<f64> {
    let now = epoch_ms();
    (reading?.usage.deciding())
        .map(|window| window.used_at(now))
        .reduce(f64::max)
}

fn ended(result: CallResult) -> CallOutcome {
    CallOutcome {
        result,
        healthy: None,
        roll_back: false,
        why: None,
        at: epoch_ms(),
    }
}

fn signed_in_by_hand(core: &Core, account: &str) -> bool {
    let book = core.workspaces().accounts().unwrap_or_default();
    (book.with_built_ins().profile(account)).is_some_and(|profile| by_hand(&profile.provider))
}

/// The person answered: the chats still on the refused account move or retry as they picked.
pub(crate) async fn answered(core: &Arc<Core>, call: &PersonCall, key: &str) {
    let Some((account, chats)) = parse(key) else {
        return;
    };
    let picked = call
        .answer
        .as_ref()
        .and_then(|answer| answer.option.clone());
    let still_on = |chat: &str| {
        super::chat::accounts(core, chat)
            .ok()
            .and_then(|takers| takers.current)
            .is_some_and(|current| current.id == account)
    };
    let chats: Vec<&String> = chats.iter().filter(|chat| still_on(chat)).collect();
    let moving_to = picked
        .as_deref()
        .and_then(|option| option.strip_prefix(MOVE));
    match (picked.as_deref(), moving_to) {
        (_, Some(to)) => {
            for chat in chats {
                if let Err(problem) = super::chat::move_to_account(core, chat, to).await {
                    eprintln!("sikemux core: a stopped chat did not move accounts: {problem}");
                }
            }
        }
        (Some(RETRY), None) => {
            for chat in chats {
                if let Ok(running) = core.chats().running(chat) {
                    let _ = running.retry_failed();
                }
            }
        }
        _ if signed_in_by_hand(core, &account) => {
            let mut left = LEFT.locked();
            left.extend(
                chats
                    .into_iter()
                    .map(|chat| (account.clone(), chat.clone())),
            );
        }
        _ => {}
    }
    person_calls::set_outcome(core, &call.id, ended(CallResult::Heard));
}
