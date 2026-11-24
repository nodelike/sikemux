//! The `sign_in` tool: a card in the agent's chat, and in its lead's, with a row for each
//! connector the person signs in to from there. Each sign-in moves its row, and once every one
//! is signed in the card folds to a line and the agent is told to carry on.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use sikemux_view::chat::{SignInCard, SignInConnector, SignInState};

use super::chief;
use super::plugins::{manifests, PluginFailure};
use super::workspace_state::WorkspaceParts;
use super::Core;
use crate::cli::protocol::HarnessRequest;
use sikemux_plugin_api::FailureCategory;

const KEY: &str = "signin|";

pub(crate) async fn tool(core: &Arc<Core>, request: &HarnessRequest) -> Result<Value, String> {
    let agent = (request.agent_id.clone()).ok_or("sign_in needs the calling agent's id")?;
    let why = (request.params.get("why").and_then(Value::as_str))
        .map(str::trim)
        .filter(|why| !why.is_empty())
        .ok_or("sign_in needs why: what you need the connectors for")?;
    let asked: Vec<&str> = (request.params.get("connectors").and_then(Value::as_array))
        .map(|asked| asked.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if asked.is_empty() {
        return Err("sign_in needs connectors: the ones whose sign-in failed".into());
    }
    let known = manifests(core);
    let mut connectors = Vec::new();
    let mut seen = BTreeSet::new();
    for named in asked {
        let found = known.iter().find(|manifest| {
            manifest.id == named
                || manifest.id.strip_prefix("sikemux.") == Some(named)
                || manifest.name.eq_ignore_ascii_case(named)
        });
        let Some(manifest) = found else {
            let names: Vec<&str> = known
                .iter()
                .map(|manifest| manifest.name.as_str())
                .collect();
            return Err(format!(
                "no connector is called {named}; the ones built in are {}",
                names.join(", ")
            ));
        };
        if seen.insert(manifest.id.clone()) {
            connectors.push(SignInConnector {
                id: manifest.id.clone(),
                name: manifest.name.clone(),
                state: SignInState::Waiting,
            });
        }
    }
    let id = (open_cards(core).into_iter())
        .find(|(_, card)| card.agent_id == agent)
        .map_or_else(
            || uuid::Uuid::new_v4().simple().to_string(),
            |(_, card)| card.id,
        );
    let name = (core.names().get(&agent)).map_or_else(|| agent.clone(), |named| named.name);
    let card = SignInCard {
        id,
        agent_id: agent.clone(),
        name,
        why: why.chars().take(200).collect(),
        connectors,
    };
    let key = format!("{KEY}{}", card.id);
    let update = sikemux_fold::update::sign_in_notice(card.clone());
    chief::put_line(core, &agent, &key, update.clone());
    if let Some(lead) = core.workspaces().agent_leads().remove(&agent) {
        chief::put_line(core, &lead, &key, update);
    }
    Ok(json!({
        "shown": true,
        "note": "The person signs in from the card in your chat. End your turn now: Sikemux messages you once every connector on it is signed in."
    }))
}

/// A sign-in of `plugin` ended, failed when `failure` says so: the cards that wait on it say how
/// it stands, and each one that has every connector back tells its agent.
pub(crate) fn moved(core: &Core, plugin: &str, failure: Option<&PluginFailure>) {
    let waiting = open_cards(core)
        .iter()
        .any(|(_, card)| (card.connectors.iter()).any(|connector| connector.id == plugin));
    let (true, Some(core), Ok(runtime)) =
        (waiting, core.arc(), tokio::runtime::Handle::try_current())
    else {
        return;
    };
    let plugin = plugin.to_owned();
    let failed = failure
        .filter(|failure| {
            !matches!(
                failure.category,
                FailureCategory::StreamClosed | FailureCategory::Stopped
            )
        })
        .map(|failure| failure.message.clone());
    runtime.spawn(async move {
        let listed = super::plugins::listed(&core).await;
        let signed_in = (listed.iter().find(|listed| listed.id == plugin))
            .and_then(|listed| listed.signed_in)
            .unwrap_or(false);
        let state = match (signed_in, failed) {
            (true, _) => SignInState::SignedIn,
            (false, Some(reason)) => SignInState::Failed { reason },
            (false, None) => return,
        };
        let mut back = Vec::new();
        chief::change_lines(&core, KEY, |update| {
            let Some(mut card) = card_of(update) else {
                return false;
            };
            let was_done = card.done();
            let mut changed = false;
            for connector in &mut card.connectors {
                if connector.id == plugin && connector.state != state {
                    connector.state = state.clone();
                    changed = true;
                }
            }
            if !changed || was_done {
                return false;
            }
            if card.done() && !back.iter().any(|done: &SignInCard| done.id == card.id) {
                back.push(card.clone());
            }
            *update = sikemux_fold::update::sign_in_notice(card);
            true
        });
        for card in back {
            let names: Vec<&str> = (card.connectors.iter())
                .map(|connector| connector.name.as_str())
                .collect();
            let text = format!(
                "The person signed in to {} again. Carry on with what you needed them for: {}",
                names.join(" and "),
                card.why
            );
            let _ = super::messages::from_sikemux(&core, card.agent_id, text).await;
        }
    });
}

/// The cards still waiting on a sign-in, once each, with a chat that holds it.
fn open_cards(core: &Core) -> Vec<(String, SignInCard)> {
    let mut seen = BTreeSet::new();
    (chief::lines_under(core, KEY).into_iter())
        .filter_map(|(agent, update)| Some((agent, card_of(&update)?)))
        .filter(|(_, card)| !card.done() && seen.insert(card.id.clone()))
        .collect()
}

fn card_of(update: &Value) -> Option<SignInCard> {
    serde_json::from_value(update.get("signIn")?.clone()).ok()
}
