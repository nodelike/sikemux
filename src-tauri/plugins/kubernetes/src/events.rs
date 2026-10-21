//! A namespace's recent events, newest first: what the cluster says went wrong,
//! such as images it couldn't pull, pods it couldn't place, or containers it killed.

use serde::Deserialize;
use serde_json::Value;

use crate::describe::{events_of, Event};
use crate::error::KubeResult;
use crate::kubectl::{self, Scope};

const DEFAULT_LIMIT: usize = 100;

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct ListEvents {
    #[serde(flatten)]
    pub scope: Scope,
    pub all_namespaces: bool,
    /// Only warnings, leaving out the routine Normal events.
    pub warnings_only: bool,
    pub limit: Option<usize>,
}

pub fn args_of(input: &ListEvents) -> Vec<&'static str> {
    let mut args = vec!["get", "events"];
    if input.all_namespaces {
        args.push("--all-namespaces");
    }
    if input.warnings_only {
        args.push("--field-selector=type=Warning");
    }
    args
}

pub async fn list(input: ListEvents) -> KubeResult<Vec<Event>> {
    let list: Value = kubectl::json(&input.scope, &args_of(&input)).await?;
    let mut events = events_of(&list);
    events.truncate(input.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, 1000));
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warnings_and_every_namespace_are_asked_of_kubectl() {
        let input = ListEvents {
            warnings_only: true,
            all_namespaces: true,
            ..ListEvents::default()
        };
        assert_eq!(
            args_of(&input),
            [
                "get",
                "events",
                "--all-namespaces",
                "--field-selector=type=Warning"
            ]
        );
        assert_eq!(args_of(&ListEvents::default()), ["get", "events"]);
    }
}
