//! Keeps the ticket behind each card in step with it through the person's
//! own sign-in: a card taken moves its ticket to the project's In Progress,
//! a card done moves it to Done with a comment naming its pull request, and
//! a ticket closed in its tracker closes its card.

use std::sync::Arc;

use serde_json::{json, Value};

use super::{change_card, send_notices, By, Changes, Status, Ticket};
use crate::server::connection::blocking;
use crate::server::plugins;
use crate::server::remote::unix_ms;
use crate::server::wake::sooner;
use crate::server::Core;

/// How often a ticket is asked whether it was closed in its tracker.
const UPSTREAM_EVERY_MS: u64 = 5 * 60 * 1000;
/// How long after a failed update the ticket is tried again.
const RETRY_AFTER_MS: u64 = 5 * 60 * 1000;

/// The tracker a ticket lives in. Only Jira has a plugin; a Linear ticket
/// waits here for one.
enum Tracker {
    Jira { site: Option<String> },
}

fn tracker(ticket: &Ticket) -> Option<Tracker> {
    match ticket.provider.as_str() {
        "jira" => Some(Tracker::Jira {
            site: ticket.url.as_deref().and_then(site_of),
        }),
        _ => None,
    }
}

/// The host a ticket's web address is on, which names its Jira site.
fn site_of(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let host = rest.split(['/', '?', '#']).next()?;
    (!host.is_empty()).then(|| host.to_owned())
}

/// Where a failed update of a ticket to `status` notes when it failed.
fn failed_key(status: Status) -> &'static str {
    match status {
        Status::Open => "open-failed",
        Status::Progress => "progress-failed",
        Status::Done => "done-failed",
    }
}

enum Work {
    /// Move the ticket to match the card, then comment.
    Push {
        to: String,
        status: Status,
        comment: Option<String>,
    },
    /// See whether the ticket was closed in its tracker.
    Look,
}

/// Starts each ticket update or look that is due, and answers when the next
/// one will be.
pub(super) fn due(core: &Arc<Core>) -> Option<u64> {
    let now = unix_ms();
    let mut next = None;
    let due: Vec<(String, u64, Ticket, Work)> = {
        let mut state = core.boards.lock();
        let mut due = Vec::new();
        for (project, board) in &state.boards {
            for card in &board.cards {
                let Some(ticket) = &card.ticket else { continue };
                if tracker(ticket).is_none() {
                    continue;
                }
                let at = |what: &'static str| {
                    state.polled.get(&(project.clone(), card.id, what)).copied()
                };
                let since =
                    |what: &'static str| at(what).map_or(u64::MAX, |at| now.saturating_sub(at));
                let wanted = match card.status {
                    Status::Open => None,
                    Status::Progress => Some(&board.settings.statuses.progress),
                    Status::Done => Some(&board.settings.statuses.done),
                };
                let work = match wanted {
                    Some(to) if ticket.synced != Some(card.status) => {
                        if since(failed_key(card.status)) < RETRY_AFTER_MS {
                            next = sooner(
                                next,
                                at(failed_key(card.status)).map(|at| at + RETRY_AFTER_MS),
                            );
                            continue;
                        }
                        let comment = (card.status == Status::Done)
                            .then(|| card.auto.as_ref().and_then(|run| run.pull.as_ref()))
                            .flatten()
                            .map(|pull| format!("Merged in {} by Sikemux's Auto.", pull.url));
                        Work::Push {
                            to: to.clone(),
                            status: card.status,
                            comment,
                        }
                    }
                    _ if card.status != Status::Done && since("look") >= UPSTREAM_EVERY_MS => {
                        Work::Look
                    }
                    _ if card.status != Status::Done => {
                        next = sooner(next, at("look").map(|at| at + UPSTREAM_EVERY_MS));
                        continue;
                    }
                    _ => continue,
                };
                if state.syncing.contains(&(project.clone(), card.id)) {
                    continue;
                }
                due.push((project.clone(), card.id, ticket.clone(), work));
            }
        }
        for (project, id, _, work) in &due {
            match work {
                Work::Push { .. } => {
                    state.syncing.insert((project.clone(), *id));
                }
                Work::Look => {
                    state.polled.insert((project.clone(), *id, "look"), now);
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
                core.boards.lock().syncing.remove(&(project, id));
                core.boards.wake.now();
            }
        });
    }
    next
}

async fn run(core: &Arc<Core>, project: &str, id: u64, ticket: &Ticket, work: Work) {
    let Some(Tracker::Jira { site }) = tracker(ticket) else {
        return;
    };
    let issue = plugins::call(
        "sikemux.jira",
        "issue",
        json!({ "key": ticket.key, "site": site }),
    )
    .await;
    match work {
        Work::Push {
            to,
            status,
            comment,
        } => {
            let outcome = match issue {
                Ok(issue) => push(&ticket.key, &site, &issue, &to, comment).await,
                Err(error) => Err(error),
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
    key: &str,
    site: &Option<String>,
    issue: &Value,
    to: &str,
    comment: Option<String>,
) -> Result<(), String> {
    let there = issue["status"]
        .as_str()
        .is_some_and(|status| status.eq_ignore_ascii_case(to));
    if !there {
        plugins::call(
            "sikemux.jira",
            "transition",
            json!({ "key": key, "to": to, "site": site }),
        )
        .await?;
    }
    if let Some(body) = comment {
        plugins::call(
            "sikemux.jira",
            "comment",
            json!({ "key": key, "body": body, "site": site }),
        )
        .await?;
    }
    Ok(())
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
    let mut state = core.boards.lock();
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
    let failed = (project.to_owned(), id, failed_key(status));
    match outcome {
        Ok(()) => {
            ticket.synced = Some(status);
            ticket.sync_note = None;
            state.polled.remove(&failed);
        }
        Err(error) => {
            ticket.sync_note = Some(format!("{key} was not updated in Jira: {error}"));
            state.polled.insert(failed, unix_ms());
        }
    }
    core.boards.save(&mut state, project);
}

/// The ticket was closed in its tracker, so its card is done.
async fn close(core: &Arc<Core>, project: &str, id: u64, key: &str) {
    let core2 = core.clone();
    let (project, key) = (project.to_owned(), key.to_owned());
    let notices = blocking(move || {
        let open = {
            let mut state = core2.boards.lock();
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
