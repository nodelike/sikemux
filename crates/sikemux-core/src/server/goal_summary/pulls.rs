//! Where the pull requests of a goal's cards stand, asked of the code host as its page opens and
//! kept on each card. A card's commits name its pull requests; an agent's last word on one goes
//! stale the moment it merges, so a merged or closed one becomes a moment of its own, and the
//! summary is written again when one changes. A lookup that fails leaves the card as it was.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use sikemux_view::slice::{SourceKind, SourceLink};

use super::moments::{fact, family, Gathered};
use crate::server::board::{self, BoardCard, CardPull, PullState};
use crate::server::codehost::repo::{self, Repo};
use crate::server::Core;
use crate::sessions::asked::prose_of;

const MAX_LOOKUPS: usize = 8;
const LOOKUP_TIME: Duration = Duration::from_secs(10);

fn written_at(found: &Value, key: &str) -> Option<u64> {
    let at = time::OffsetDateTime::parse(
        found.get(key)?.as_str()?,
        &time::format_description::well_known::Rfc3339,
    );
    u64::try_from(at.ok()?.unix_timestamp_nanos() / 1_000_000).ok()
}

async fn ask(repo: &Repo, method: &str, more: Value) -> Option<Value> {
    (tokio::time::timeout(LOOKUP_TIME, repo.call(method, more)).await)
        .ok()?
        .ok()
}

/// The pull request the code host answered with, and how its checks stand while it is open.
async fn card_pull(repo: &Repo, found: &Value) -> Option<CardPull> {
    let state = match found["state"].as_str()? {
        "merged" => PullState::Merged,
        "closed" => PullState::Closed,
        _ => PullState::Open,
    };
    let checks = match state {
        PullState::Open => tokio::time::timeout(LOOKUP_TIME, repo::pull_checks(repo, found))
            .await
            .ok()
            .and_then(Result::ok)
            .flatten(),
        PullState::Merged | PullState::Closed => None,
    };
    Some(CardPull {
        number: found["number"].as_u64()?,
        title: prose_of(found["title"].as_str().unwrap_or_default()),
        url: found["url"].as_str().unwrap_or_default().to_owned(),
        state,
        checks,
        at: written_at(found, "mergedAt")
            .or_else(|| written_at(found, "closedAt"))
            .or_else(|| written_at(found, "updatedAt"))?,
    })
}

/// What the code host says now of `card`'s open pull requests, or of those its newest commit
/// not on the project's branch went up in when it has none open.
async fn looked(repo: &Repo, card: &BoardCard, lookups: &mut usize) -> Vec<Value> {
    let open: Vec<u64> = (card.pulls.iter())
        .filter(|pull| pull.state == PullState::Open)
        .map(|pull| pull.number)
        .collect();
    let mut found = Vec::new();
    if open.is_empty() {
        let Some(newest) = card.commits.iter().rev().find(|commit| !commit.landed) else {
            return found;
        };
        *lookups -= 1;
        let pulls = ask(repo, "commitPulls", json!({ "sha": newest.sha })).await;
        found.extend(
            pulls
                .as_ref()
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .cloned(),
        );
        return found;
    }
    for number in open {
        if *lookups == 0 {
            break;
        }
        *lookups -= 1;
        found.extend(ask(repo, "pull", json!({ "number": number })).await);
    }
    found
}

fn wants_lookup(card: &BoardCard) -> bool {
    (card.pulls.iter()).any(|pull| pull.state == PullState::Open)
        || (card.commits.iter()).any(|commit| !commit.landed)
}

/// Records where the pull requests of the goal's cards stand, and adds each that merged or
/// closed to `gathered`.
pub(super) async fn add(core: &Arc<Core>, project: &str, gathered: &mut Gathered) {
    let view = board::project_view(core, project);
    let cards: Vec<&BoardCard> = family(&view.cards, gathered.card);
    let mut pulls: Vec<CardPull> = (cards.iter()).flat_map(|card| card.pulls.clone()).collect();
    if cards.iter().any(|card| wants_lookup(card)) {
        let found = tokio::time::timeout(LOOKUP_TIME, repo::of_project(core, project)).await;
        if let Some(repo) = found.ok().and_then(Result::ok) {
            let mut lookups = MAX_LOOKUPS;
            for card in cards.iter().filter(|card| wants_lookup(card)) {
                if lookups == 0 {
                    break;
                }
                for found in looked(&repo, card, &mut lookups).await {
                    let Some(pull) = card_pull(&repo, &found).await else {
                        continue;
                    };
                    board::pull_seen(core, project, card.id, pull.clone());
                    pulls.retain(|known| known.number != pull.number);
                    pulls.push(pull);
                }
            }
        }
    }
    for pull in pulls {
        let verb = match pull.state {
            PullState::Merged => "was merged",
            PullState::Closed => "was closed without merging",
            PullState::Open => continue,
        };
        let words = format!("Pull request #{} ({}) {verb}.", pull.number, pull.title);
        let link = match pull.url.is_empty() {
            true => SourceLink::Card {
                card: gathered.card,
            },
            false => SourceLink::Url { url: pull.url },
        };
        gathered.insert(
            fact(SourceKind::PullRequest, pull.at, None, &words, link),
            Some(words),
        );
    }
}
