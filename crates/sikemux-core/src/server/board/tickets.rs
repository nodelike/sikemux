//! Keeps the ticket behind each card in step with it through the person's
//! own sign-in: a card taken moves its ticket into the workflow's in-progress
//! statuses, a card done moves it into its done statuses with a comment naming
//! its pull request, and a ticket closed in its tracker closes its card.

use std::sync::Arc;

use serde_json::{json, Value};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use super::{change_card, send_notices, By, Changes, Status, Ticket, Tracker};
use crate::server::connection::blocking;
use crate::server::plugins::{self, Failure, Provided};
use crate::server::wake::sooner;
use crate::server::Core;

/// How often a ticket is asked whether it was closed in its tracker.
const UPSTREAM_EVERY_MS: u64 = 5 * 60 * 1000;
/// How long after a failed update the ticket is tried again.
const RETRY_AFTER_MS: u64 = 5 * 60 * 1000;

/// A ticket in Jira, the one tracker with a plugin; a Linear ticket waits
/// for one.
struct Jira<'a> {
    plugins: Option<Provided>,
    key: &'a str,
    /// The Jira site, which the ticket's web address names.
    site: Option<String>,
}

impl<'a> Jira<'a> {
    fn of(core: &Core, ticket: &'a Ticket) -> Option<Self> {
        (ticket.provider == Tracker::Jira).then(|| Self {
            plugins: core.plugins,
            key: &ticket.key,
            site: ticket.url.as_deref().and_then(site_of),
        })
    }

    async fn call(&self, method: &str, mut params: Value) -> Result<Value, Failure> {
        params["key"] = self.key.into();
        params["site"] = json!(self.site);
        plugins::call_on(self.plugins, "sikemux.jira", method, params).await
    }
}

/// The host a ticket's web address is on, which names its Jira site.
fn site_of(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let host = rest.split(['/', '?', '#']).next()?;
    (!host.is_empty()).then(|| host.to_owned())
}

enum Work {
    /// Move the ticket to match the card, then comment.
    Push {
        /// The Jira status category the ticket should be in.
        to: &'static str,
        status: Status,
        comment: Option<String>,
    },
    /// See whether the ticket was closed in its tracker.
    Look,
}

/// Starts each ticket update or look that is due, and answers when the next
/// one will be.
pub(super) fn due(core: &Arc<Core>) -> Option<u64> {
    let now = epoch_ms();
    let mut next = None;
    let due: Vec<(String, u64, Ticket, Work)> = {
        let mut state = core.boards.state.locked();
        let mut due = Vec::new();
        for (project, board) in &state.boards {
            for card in &board.cards {
                let Some(ticket) = &card.ticket else { continue };
                if Jira::of(core, ticket).is_none() {
                    continue;
                }
                let kept = state.work_of(project, card.id);
                let failed = kept
                    .and_then(|kept| kept.ticket_failed)
                    .filter(|(status, _)| *status == card.status)
                    .map(|(_, at)| at);
                let looked = kept.and_then(|kept| kept.ticket_looked);
                let since = |at: Option<u64>| at.map_or(u64::MAX, |at| now.saturating_sub(at));
                let wanted = match card.status {
                    Status::Open => None,
                    Status::Progress => Some("indeterminate"),
                    Status::Done => Some("done"),
                };
                let work = match wanted {
                    Some(to) if ticket.synced != Some(card.status) => {
                        if since(failed) < RETRY_AFTER_MS {
                            next = sooner(next, failed.map(|at| at + RETRY_AFTER_MS));
                            continue;
                        }
                        let comment = (card.status == Status::Done)
                            .then(|| card.auto.as_ref().and_then(|run| run.pull.as_ref()))
                            .flatten()
                            .map(|pull| format!("Merged in {} by Sikemux's Auto.", pull.url));
                        Work::Push {
                            to,
                            status: card.status,
                            comment,
                        }
                    }
                    _ if card.status != Status::Done && since(looked) >= UPSTREAM_EVERY_MS => {
                        Work::Look
                    }
                    _ if card.status != Status::Done => {
                        next = sooner(next, looked.map(|at| at + UPSTREAM_EVERY_MS));
                        continue;
                    }
                    _ => continue,
                };
                if kept.is_some_and(|kept| kept.syncing) {
                    continue;
                }
                due.push((project.clone(), card.id, ticket.clone(), work));
            }
        }
        for (project, id, _, work) in &due {
            match work {
                Work::Push { .. } => {
                    state.work(project, *id).syncing = true;
                }
                Work::Look => {
                    state.work(project, *id).ticket_looked = Some(now);
                    next = sooner(next, Some(now + UPSTREAM_EVERY_MS));
                }
            }
        }
        due
    };
    for (project, id, ticket, work) in due {
        let core = core.clone();
        tokio::spawn(async move {
            let pushing = matches!(work, Work::Push { .. });
            run(&core, &project, id, &ticket, work).await;
            if pushing {
                core.boards
                    .state
                    .locked()
                    .change_work(&project, id, |kept| kept.syncing = false);
                core.boards.wake.now();
            }
        });
    }
    next
}

async fn run(core: &Arc<Core>, project: &str, id: u64, ticket: &Ticket, work: Work) {
    let Some(jira) = Jira::of(core, ticket) else {
        return;
    };
    let issue = jira.call("issue", json!({})).await;
    match work {
        Work::Push {
            to,
            status,
            comment,
        } => {
            let outcome = match issue {
                Ok(issue) => push(&jira, &issue, to, comment).await,
                Err(error) => Err(error.into()),
            };
            settle(core, project, id, &ticket.key, status, outcome);
        }
        Work::Look => {
            let closed = issue
                .ok()
                .is_some_and(|issue| issue["statusCategory"].as_str() == Some("done"));
            if closed {
                close(core, project, id, &ticket.key).await;
            }
        }
    }
}

async fn push(
    jira: &Jira<'_>,
    issue: &Value,
    category: &str,
    comment: Option<String>,
) -> Result<(), String> {
    if issue["statusCategory"].as_str() != Some(category) {
        let offered = jira.call("transition", json!({})).await?;
        let Some(id) = into_category(&offered["transitions"], category) else {
            let into = if category == "done" {
                "a done"
            } else {
                "an in-progress"
            };
            return Err(format!(
                "its workflow has no way into {into} status from where it is now"
            ));
        };
        jira.call("transition", json!({ "to": id })).await?;
    }
    if let Some(body) = comment {
        jira.call("comment", json!({ "body": body })).await?;
    }
    Ok(())
}

/// The transition into `category` a person would take: the one into the
/// status named for it, such as In Progress or Done, else the first offered.
fn into_category(transitions: &Value, category: &str) -> Option<String> {
    let usual = if category == "done" {
        "done"
    } else {
        "in progress"
    };
    let into: Vec<&Value> = transitions
        .as_array()?
        .iter()
        .filter(|transition| transition["toCategory"].as_str() == Some(category))
        .collect();
    into.iter()
        .find(|transition| {
            transition["to"]
                .as_str()
                .is_some_and(|to| to.eq_ignore_ascii_case(usual))
        })
        .or(into.first())
        .and_then(|transition| transition["id"].as_str())
        .map(str::to_owned)
}

/// Notes on the card's ticket how its update went.
fn settle(
    core: &Core,
    project: &str,
    id: u64,
    key: &str,
    status: Status,
    outcome: Result<(), String>,
) {
    let mut state = core.boards.state.locked();
    let Some(card) = state
        .boards
        .get_mut(project)
        .and_then(|board| board.card_mut(id).ok())
    else {
        return;
    };
    let Some(ticket) = card.ticket.as_mut().filter(|ticket| ticket.key == key) else {
        return;
    };
    let failed = match outcome {
        Ok(()) => {
            ticket.synced = Some(status);
            ticket.sync_note = None;
            None
        }
        Err(error) => {
            ticket.sync_note = Some(format!("{key} was not updated in Jira: {error}"));
            Some((status, epoch_ms()))
        }
    };
    state.change_work(project, id, |kept| kept.ticket_failed = failed);
    core.boards.save(&mut state, project);
}

/// The ticket was closed in its tracker, so its card is done.
async fn close(core: &Arc<Core>, project: &str, id: u64, key: &str) {
    let core2 = core.clone();
    let (project, key) = (project.to_owned(), key.to_owned());
    let notices = blocking(move || {
        let open = {
            let mut state = core2.boards.state.locked();
            let Some(card) = state
                .boards
                .get_mut(&project)
                .and_then(|board| board.card_mut(id).ok())
            else {
                return Ok(Vec::new());
            };
            let Some(ticket) = card.ticket.as_mut().filter(|ticket| ticket.key == key) else {
                return Ok(Vec::new());
            };
            ticket.synced = Some(Status::Done);
            card.status != Status::Done
        };
        if !open {
            return Ok(Vec::new());
        }
        let changes = Changes {
            status: Some(Status::Done),
            ..Changes::default()
        };
        change_card(&core2, &project, &By::Auto("Jira"), id, changes)
            .map(|(_, notices)| notices)
            .map_err(Into::into)
    })
    .await
    .unwrap_or_default();
    send_notices(core, notices);
}
