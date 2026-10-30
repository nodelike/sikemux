//! The CI the top bar's chip shows: the repository and branch watched, the commit its newest
//! run is for, and that commit's checks.

use std::collections::HashSet;

use gpui_kit::Entity;
use serde::Deserialize;
use serde_json::{json, Value};
use sikemux_plugin_api::codehost::Run;
use sikemux_wire::changed::Topic;
use tokio::sync::mpsc::UnboundedSender;

use crate::core::{Ask, Watch};

pub struct Ci {
    /// The repository and branch whose latest CI run the top bar shows.
    pub(crate) watched: Option<CiRepo>,
    pub(crate) card: Entity<crate::ci_card::CiCard>,
    /// The commit whose runs from every workflow, tags' included, the glyph sums up.
    head: Option<String>,
    /// The checks of `head`, as `head_runs` describes them.
    runs: Vec<Run>,
}

#[derive(Clone, PartialEq)]
pub struct CiRepo {
    /// The plugin that speaks for the code host, such as `sikemux.github`.
    pub plugin: String,
    pub owner: String,
    pub name: String,
    pub account: Option<String>,
    pub branch: String,
}

/// What the code host plugin's `runs` answered.
#[derive(Deserialize)]
struct Answer {
    value: Page,
}

#[derive(Deserialize)]
struct Page {
    runs: Vec<Run>,
}

fn runs(answer: Value) -> Vec<Run> {
    serde_json::from_value::<Answer>(answer).map_or_else(|_| Vec::new(), |it| it.value.runs)
}

impl Ci {
    pub fn new(card: Entity<crate::ci_card::CiCard>) -> Self {
        Self {
            watched: None,
            card,
            head: None,
            runs: Vec::new(),
        }
    }

    /// Keeps the core checking the CI of `shown`, or of none.
    pub fn watch(&mut self, shown: Option<CiRepo>, asks: &UnboundedSender<Ask>) {
        if shown == self.watched {
            return;
        }
        let repos: Vec<Value> = shown
            .iter()
            .map(|ci| json!({ "provider": ci.plugin, "owner": ci.owner, "name": ci.name, "account": ci.account, "host": null }))
            .collect();
        let _ = asks.send(Ask::Watch(
            "ci-shown",
            Watch::new("showCi", json!({ "repos": repos }), &[]),
        ));
        if let Some(ci) = &shown {
            let runs = json!({ "owner": ci.owner, "name": ci.name, "account": ci.account, "branch": ci.branch, "perPage": 1 });
            let call = json!({ "plugin": ci.plugin, "method": "runs", "params": runs });
            let _ = asks.send(Ask::Watch("ci", Watch::new("plugin", call, &[Topic::Ci])));
        }
        self.watched = shown;
    }

    /// Takes the branch's newest run and watches every run of the commit it is for, which
    /// also finds those a tag on it started. Says whether that commit changed.
    pub fn newest_read(&mut self, answer: Value, asks: &UnboundedSender<Ask>) -> bool {
        let head = runs(answer).into_iter().next().map(|run| run.sha);
        let Some(ci) = self.watched.as_ref().filter(|_| head.is_some()) else {
            return false;
        };
        if head == self.head {
            return false;
        }
        let runs = json!({ "owner": ci.owner, "name": ci.name, "account": ci.account, "headSha": head, "perPage": 20 });
        let call = json!({ "plugin": ci.plugin, "method": "runs", "params": runs });
        let _ = asks.send(Ask::Watch(
            "ci-head",
            Watch::new("plugin", call, &[Topic::Ci]),
        ));
        self.head = head;
        self.runs.clear();
        true
    }

    /// Keeps the head commit's runs the core read, and says whether they changed.
    pub fn head_read(&mut self, answer: Value) -> bool {
        let Some(head) = &self.head else {
            return false;
        };
        let mut seen = HashSet::new();
        let runs: Vec<Run> = runs(answer)
            .into_iter()
            .filter(|run| &run.sha == head)
            .filter(|run| {
                matches!(
                    run.event.as_str(),
                    "push" | "pull_request" | "pull_request_target" | "merge_group"
                )
            })
            .filter(|run| seen.insert((run.workflow_id.clone(), run.branch.clone())))
            .collect();
        if runs == self.runs {
            return false;
        }
        self.runs = runs;
        true
    }

    /// The branch's newest commit's checks, while the core is checking it: the latest run of
    /// each workflow a push or pull request started, as the code host counts a commit's checks.
    /// Runs started by hand or on a schedule only happen to sit on the commit, so they are left out.
    pub fn head_runs(&self) -> &[Run] {
        match self.watched {
            Some(_) => &self.runs,
            None => &[],
        }
    }
}
