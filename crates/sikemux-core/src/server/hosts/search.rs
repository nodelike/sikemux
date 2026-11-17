//! A search of chats across the account's linked hosts. Each host answers with what its own
//! search finds, ranked, and this host merges their hits by rank as they come, naming the
//! hosts that did not answer in time.

use crate::server::parts::Parts;
use crate::server::remote_state::RemoteParts;
use crate::server::workspace_state::WorkspaceParts;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sikemux_agents::sessions::said::FoundLine;
use sikemux_calls::workspace::{RecentChats, RecentChatsParams};
use sikemux_corekit::accounts::{allows, protocol::PlanFeature};
use sikemux_view::device::{FoundChat, RecentInfo};
use tokio::task::JoinSet;

use super::super::chat_search::{search_here, Rank};
use super::super::Core;
use super::ask;
use super::calls::{self as asks, params_of, HostAsk};
use super::link::Answer;

/// The longest a search waits for the linked hosts; a host slower than this is named as one
/// that did not answer.
const SEARCH_WAIT: Duration = Duration::from_secs(3);

#[derive(Serialize, Deserialize)]
pub(crate) struct SearchParams {
    query: String,
    /// Only the chats of the project of this repository, when set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    limit: Option<usize>,
}

/// What a host's own search found, each hit with its rank there.
#[derive(Default, Serialize, Deserialize)]
pub(crate) struct FoundThere {
    found: Vec<(Rank, FoundChat)>,
    chats: Vec<RecentInfo>,
    said: HashMap<String, FoundLine>,
}

/// What the linked hosts find for `query`, scoped as `asked` is, merged by rank.
pub(crate) async fn elsewhere(
    core: &Arc<Core>,
    asked: &RecentChatsParams,
    query: &str,
) -> RecentChats {
    if !allows(core.account().shown().as_ref(), PlanFeature::HistorySearch) {
        return RecentChats::default();
    }
    let identity = match &asked.project {
        None => None,
        Some(project) => {
            let identity = (core.workspaces().projects().into_iter())
                .find(|known| &known.id == project)
                .and_then(|known| known.identity);
            match identity {
                Some(identity) => Some(identity),
                None => return RecentChats::default(),
            }
        }
    };
    let mut answer = RecentChats::default();
    let mut asking = JoinSet::new();
    let mut waiting = Vec::new();
    for link in core.remote().links() {
        if !asked.hosts.is_empty() && !asked.hosts.contains(&link.id) {
            continue;
        }
        let holds = (identity.as_deref())
            .is_none_or(|identity| core.hosts().copies.project_on(&link.id, identity).is_some());
        if !holds {
            continue;
        }
        if !core.hosts().reachable(&link.id) {
            answer.unanswered.push(link.id);
            continue;
        }
        let params = params_of::<asks::SearchChats>(&SearchParams {
            query: query.to_owned(),
            identity: identity.clone(),
            limit: asked.limit,
        });
        let (core, host) = (core.clone(), link.id.clone());
        waiting.push(link.id);
        asking.spawn(async move {
            let asked = ask(
                &core,
                &host,
                asks::SearchChats::METHOD,
                params,
                None,
                SEARCH_WAIT,
            );
            let found = tokio::time::timeout(SEARCH_WAIT, asked).await.ok()?.ok()?;
            Some((host, serde_json::from_value::<FoundThere>(found).ok()?))
        });
    }
    let until = tokio::time::Instant::now() + SEARCH_WAIT;
    let mut found = Vec::new();
    while let Ok(Some(done)) = tokio::time::timeout_at(until, asking.join_next()).await {
        let Ok(Some((host, there))) = done else {
            continue;
        };
        waiting.retain(|waited| *waited != host);
        let there_found = (there.found.into_iter()).map(|(rank, hit)| {
            (
                rank,
                FoundChat {
                    host: Some(host.clone()),
                    ..hit
                },
            )
        });
        found = merged(found, there_found.collect());
        answer.chats.extend(there.chats);
        answer.said.extend(there.said);
    }
    asking.abort_all();
    answer.unanswered.extend(waiting);
    answer.found = found.into_iter().map(|(_, hit)| hit).collect();
    answer
}

/// `first` and `then` as one list by rank, each keeping its own order, `first` ahead on a tie.
fn merged(first: Vec<(Rank, FoundChat)>, then: Vec<(Rank, FoundChat)>) -> Vec<(Rank, FoundChat)> {
    let mut all = Vec::with_capacity(first.len() + then.len());
    let (mut first, mut then) = (first.into_iter().peekable(), then.into_iter().peekable());
    loop {
        let take_first = match (first.peek(), then.peek()) {
            (Some(one), Some(other)) => one.0 <= other.0,
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => return all,
        };
        all.extend(match take_first {
            true => first.next(),
            false => then.next(),
        });
    }
}

/// Searches this host for a linked host, in its copy of the project the search is scoped to.
pub(super) async fn here(core: &Arc<Core>, params: SearchParams) -> Answer {
    let project = match &params.identity {
        None => None,
        Some(identity) => {
            let project = (core.workspaces().projects().into_iter())
                .find(|known| known.identity.as_deref() == Some(identity.as_str()));
            match project {
                Some(project) => Some(project.id),
                None => return to_answer(&FoundThere::default()),
            }
        }
    };
    let asked = RecentChatsParams {
        project,
        query: Some(params.query),
        cursor: None,
        limit: params.limit,
        quick: false,
        elsewhere: false,
        hosts: Vec::new(),
    };
    let (answer, found) = search_here(core, asked)
        .await
        .map_err(|error| error.to_string())?;
    to_answer(&FoundThere {
        found,
        chats: answer.chats,
        said: answer.said,
    })
}

fn to_answer(found: &FoundThere) -> Answer {
    serde_json::to_value(found).map_err(|error| error.to_string())
}
