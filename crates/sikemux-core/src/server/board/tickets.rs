//! Keeps the ticket behind each card in step with it through the person's
//! own sign-in: a card taken moves its ticket into the workflow's in-progress
//! statuses, a card done moves it into its done statuses with a comment naming
//! its pull request, and a ticket closed in its tracker closes its card, but
//! for a slice the person lands that has not landed, which waits for them.

use crate::server::parts::Parts;
use std::sync::Arc;

use serde_json::{json, Value};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;

use super::auto::Landing;
use super::{
    add_card, change_card, chars, clipped, distinct_cards, long_text, project_at, project_view,
    send_notices, title_of, Adding, BoardAddTicketParams, BoardTicketParams, By, CardStatus,
    CardTicket, Changes, Notice, ProjectBoard, Tracker, MAX_TITLE_CHARS,
};
use crate::server::connection::blocking;
use crate::server::plugins::{self, PluginFailure, Provided};
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
    fn of(core: &Core, ticket: &'a CardTicket) -> Option<Self> {
        (ticket.provider == Tracker::Jira).then(|| Self {
            plugins: core.config().plugins,
            key: &ticket.key,
            site: ticket.url.as_deref().and_then(site_of),
        })
    }

    async fn call(&self, method: &str, mut params: Value) -> Result<Value, PluginFailure> {
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
        status: CardStatus,
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
    let due: Vec<(String, u64, CardTicket, Work)> = {
        let mut state = core.boards().state.locked();
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
                    CardStatus::Open => None,
                    CardStatus::Progress => Some("indeterminate"),
                    CardStatus::Done => Some("done"),
                };
                let work = match wanted {
                    Some(to) if ticket.synced != Some(card.status) && !ticket.done_unlanded => {
                        if since(failed) < RETRY_AFTER_MS {
                            next = sooner(next, failed.map(|at| at + RETRY_AFTER_MS));
                            continue;
                        }
                        let comment = (card.status == CardStatus::Done)
                            .then(|| card.auto.as_ref().and_then(|run| run.pull.as_ref()))
                            .flatten()
                            .map(|pull| format!("Merged in {} by Sikemux's Auto.", pull.url));
                        Work::Push {
                            to,
                            status: card.status,
                            comment,
                        }
                    }
                    _ if card.status != CardStatus::Done && since(looked) >= UPSTREAM_EVERY_MS => {
                        Work::Look
                    }
                    _ if card.status != CardStatus::Done => {
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
                core.boards()
                    .state
                    .locked()
                    .change_work(&project, id, |kept| kept.syncing = false);
                core.boards().wake.now();
            }
        });
    }
    next
}

async fn run(core: &Arc<Core>, project: &str, id: u64, ticket: &CardTicket, work: Work) {
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
            let category = issue
                .ok()
                .and_then(|issue| issue["statusCategory"].as_str().map(str::to_owned));
            match category.as_deref() {
                Some("done") => close(core, project, id, &ticket.key).await,
                Some(_) if ticket.done_unlanded => reopened(core, project, id, &ticket.key),
                _ => {}
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

/// The ticket closed with nothing landed was opened again in its tracker, so
/// it no longer waits for the person, and goes back in step with its card.
fn reopened(core: &Core, project: &str, id: u64, key: &str) {
    core.boards().edit_if(project, id, |card| {
        let ticket = card.ticket.as_mut().filter(|ticket| ticket.key == key)?;
        ticket.done_unlanded = false;
        ticket.synced = None;
        Some(())
    });
}

/// Notes on the card's ticket how its update went.
fn settle(
    core: &Core,
    project: &str,
    id: u64,
    key: &str,
    status: CardStatus,
    outcome: Result<(), String>,
) {
    let mut state = core.boards().state.locked();
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
    core.boards().save(&mut state, project);
}

/// The ticket was closed in its tracker, so its card is done. A slice the
/// person lands that has not landed stays open, waiting for the person to
/// decide, since a closed ticket lands nothing.
async fn close(core: &Arc<Core>, project: &str, id: u64, key: &str) {
    let core2 = core.clone();
    let (project, key) = (project.to_owned(), key.to_owned());
    let notices = blocking(move || {
        let open = {
            let mut state = core2.boards().state.locked();
            let Some(board) = state.boards.get_mut(&project) else {
                return Ok(Vec::new());
            };
            let landing = board.settings.landing;
            let Ok(card) = board.card_mut(id) else {
                return Ok(Vec::new());
            };
            let unlanded = (card.slice.as_ref())
                .and_then(|slice| slice.waiting())
                .is_some_and(|step| landing == Landing::Person || step.environment().is_some());
            let open = card.status != CardStatus::Done;
            let Some(ticket) = card.ticket.as_mut().filter(|ticket| ticket.key == key) else {
                return Ok(Vec::new());
            };
            ticket.synced = Some(CardStatus::Done);
            if open && unlanded {
                if !ticket.done_unlanded {
                    ticket.done_unlanded = true;
                    card.updated_at = epoch_ms();
                    core2.boards().save(&mut state, &project);
                }
                return Ok(Vec::new());
            }
            open
        };
        if !open {
            return Ok(Vec::new());
        }
        let changes = Changes {
            status: Some(CardStatus::Done),
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

const MAX_TICKET_DETAILS_CHARS: usize = 4000;

/// A ticket made a card: its title on one line and its description cut to
/// what a card holds.
fn ticket_card(
    title: &str,
    details: &str,
    ticket: Option<BoardTicketParams>,
) -> Result<Adding, String> {
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    let details = if chars(details) <= MAX_TICKET_DETAILS_CHARS {
        details.to_owned()
    } else {
        let kept: String = details.chars().take(MAX_TICKET_DETAILS_CHARS).collect();
        format!("{kept}\n\n[Cut short; the ticket has the rest.]")
    };
    Ok(Adding {
        title: title_of(&clipped(&title, MAX_TITLE_CHARS))?,
        details: long_text("details", &details)?,
        ticket: ticket.map(BoardTicketParams::ticket).transpose()?,
        ..Adding::default()
    })
}

/// The card on `project`'s board for ticket `key`, an open one before a done
/// one.
fn card_of_ticket(core: &Core, project: &str, key: &str) -> Option<TicketHeld> {
    let state = core.boards().state.locked();
    let card = (state.boards.get(project)?.cards.iter())
        .filter(|card| {
            card.ticket
                .as_ref()
                .is_some_and(|ticket| ticket.key.eq_ignore_ascii_case(key))
        })
        .min_by_key(|card| card.status == CardStatus::Done)?;
    let parent_done = card
        .parent
        .and_then(|parent| state.boards.get(project)?.card(parent).ok())
        .is_some_and(|parent| parent.status == CardStatus::Done);
    Some(TicketHeld {
        id: card.id,
        parent: card.parent,
        parent_done,
        depends_on: card.depends_on.clone(),
        done: card.status == CardStatus::Done,
    })
}

struct TicketHeld {
    id: u64,
    parent: Option<u64>,
    parent_done: bool,
    depends_on: Vec<u64>,
    done: bool,
}

/// The person puts a ticket on the board: its subtasks go under it, it waits
/// on the tickets that block it and the ones it blocks wait on it, when those
/// are on the board too. Answers with the board, and what could not be done
/// besides adding the ticket.
pub(crate) async fn person_add_ticket(
    core: &Arc<Core>,
    params: BoardAddTicketParams,
) -> Result<ProjectBoard, String> {
    let (view, notices) = {
        let core = core.clone();
        blocking(move || {
            let project = project_at(&core, &params.project)?;
            let key = params.ticket.key.clone();
            add_card(
                &core,
                &project,
                &By::Person,
                ticket_card(&params.title, &params.details, Some(params.ticket))?,
                None,
            )?;
            let made = card_of_ticket(&core, &project, &key).ok_or("The ticket's card is gone")?;
            let (made, waited) = (made.id, made.depends_on);
            let mut notices = Vec::new();
            let mut problems = Vec::new();
            let change = |id: u64, changes: Changes| {
                change_card(&core, &project, &By::Person, id, changes).map(|(_, sent)| sent)
            };
            let mut note = |done: Result<Vec<Notice>, String>, what: String| match done {
                Ok(sent) => notices.extend(sent),
                Err(error) => problems.push(format!("Could not {what}: {error}")),
            };
            for sub in params.subtasks {
                let Some(sub_key) = sub.ticket.as_ref().map(|ticket| ticket.key.clone()) else {
                    continue;
                };
                match card_of_ticket(&core, &project, &sub_key) {
                    None => note(
                        ticket_card(&sub.title, &sub.details, sub.ticket).and_then(|mut adding| {
                            adding.parent = Some(made);
                            add_card(&core, &project, &By::Person, adding, None).map(|_| Vec::new())
                        }),
                        format!("put {sub_key} under it"),
                    ),
                    Some(sub) if !sub.done && (sub.parent.is_none() || sub.parent_done) => note(
                        change(
                            sub.id,
                            Changes {
                                parent: Some(Some(made)),
                                ..Changes::default()
                            },
                        ),
                        format!("put {sub_key} under it"),
                    ),
                    Some(_) => {}
                }
            }
            let blockers: Vec<u64> = params
                .blocked_by
                .iter()
                .filter_map(|blocker| card_of_ticket(&core, &project, blocker).map(|held| held.id))
                .filter(|id| !waited.contains(id))
                .collect();
            if !blockers.is_empty() {
                let waits = waited.into_iter().chain(blockers).collect();
                note(
                    distinct_cards(waits, "dependsOn").and_then(|waits| {
                        change(
                            made,
                            Changes {
                                depends_on: Some(waits),
                                ..Changes::default()
                            },
                        )
                    }),
                    "make it wait on its blockers".into(),
                );
            }
            for blocked in params.blocks {
                if let Some(held) = card_of_ticket(&core, &project, &blocked) {
                    let mut depends = held.depends_on;
                    if !depends.contains(&made) {
                        depends.push(made);
                        note(
                            change(
                                held.id,
                                Changes {
                                    depends_on: Some(depends),
                                    ..Changes::default()
                                },
                            ),
                            format!("make {blocked} wait on it"),
                        );
                    }
                }
            }
            let mut view = project_view(&core, &project);
            view.problems = problems;
            Ok((view, notices))
        })
        .await
        .map_err(|error| error.to_string())?
    };
    send_notices(core, notices);
    Ok(view)
}
