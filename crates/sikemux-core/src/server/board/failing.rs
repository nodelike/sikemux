//! CI that fails the same way on two runs in a row of the project's own
//! branch becomes work on the board. What failed is known by its check and
//! test, so the open card that holds one takes it again, and what one commit
//! broke goes on one card.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sikemux_process::sync::Locked;

use super::slice::short;
use super::{add_card, tell_clients, Adding, Board, By, MAX_TITLE_CHARS};
use crate::server::codehost::repo::FailedJob;
use crate::server::Core;
use crate::text::clipped;

/// Failures kept from one run, and on one card.
const MAX_FAILURES: usize = 8;
const MAX_OUTPUT_CHARS: usize = 4000;
const PROMISE: &str = "CI passes on the project's branch again";

/// A CI job that failed, and the test in it when its log names one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Failure {
    /// The workflow and the job, such as `CI / clippy`.
    check: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    test: Option<String>,
}

impl Failure {
    /// Whether the two name the same failure, one perhaps more closely: a
    /// workflow covers its jobs, and a job without a test covers its tests,
    /// for when a run's jobs or a job's log could not be read.
    fn same(&self, other: &Failure) -> bool {
        self == other || self.covers(other) || other.covers(self)
    }

    fn covers(&self, other: &Failure) -> bool {
        self.test.is_none()
            && (self.check == other.check
                || (other.check.strip_prefix(self.check.as_str()))
                    .is_some_and(|job| job.starts_with(" / ")))
    }

    fn label(&self) -> String {
        match &self.test {
            Some(test) => format!("{}: {test}", self.check),
            None => self.check.clone(),
        }
    }
}

/// What failed in CI on the project's branch on two runs in a row, as of `sha`.
pub(super) struct Broke {
    pub(super) failures: Vec<Failure>,
    pub(super) sha: String,
    pub(super) output: String,
}

impl Broke {
    /// The checks that failed, as one run's command.
    pub(super) fn checks(&self) -> String {
        let mut checks: Vec<&str> = (self.failures.iter())
            .map(|failure| failure.check.as_str())
            .collect();
        checks.dedup();
        checks.join(", ")
    }

    pub(super) fn held_by(&self, card: &[Failure]) -> bool {
        (self.failures.iter()).any(|failure| card.iter().any(|held| held.same(failure)))
    }

    /// Keeps its failures on `card`'s, up to as many as a card holds.
    pub(super) fn hold(&self, card: &mut Vec<Failure>) {
        for failure in &self.failures {
            if card.len() < MAX_FAILURES && !card.contains(failure) {
                card.push(failure.clone());
            }
        }
    }

    /// What whoever answers for the card that holds it is told.
    pub(super) fn said(&self) -> String {
        format!(
            "CI failed again on {} with {}",
            short(&self.sha),
            labels(&self.failures)
        )
    }
}

/// The latest failing CI run on a project's branch, kept while the core runs.
#[derive(Default)]
pub(super) struct Latest {
    sha: String,
    failures: Vec<Failure>,
}

impl Latest {
    /// Forgets the failures of a card that is done or deleted, so they need
    /// two failing runs again before they make a card.
    pub(super) fn forget(&mut self, card: &[Failure]) {
        (self.failures).retain(|failure| !card.iter().any(|held| held.same(failure)));
    }
}

fn labels(failures: &[Failure]) -> String {
    failures
        .iter()
        .map(Failure::label)
        .collect::<Vec<_>>()
        .join(", ")
}

fn failures(jobs: &[FailedJob]) -> Vec<Failure> {
    let mut failures: Vec<Failure> = jobs
        .iter()
        .flat_map(|job| {
            let tests: Vec<Option<String>> = match job.tests.is_empty() {
                true => vec![None],
                false => job.tests.iter().cloned().map(Some).collect(),
            };
            tests.into_iter().map(|test| Failure {
                check: job.check.clone(),
                test,
            })
        })
        .collect();
    let mut seen = Vec::new();
    failures.retain(|failure| {
        let new = !seen.contains(failure);
        seen.push(failure.clone());
        new
    });
    failures.truncate(MAX_FAILURES);
    failures
}

/// CI ruled on `sha`, the pushed tip of `project`'s branch: it passed when
/// no `jobs` failed. What failed on the run before too goes on a card.
pub(crate) fn ci_ruled(core: &Arc<Core>, project: &str, sha: &str, jobs: &[FailedJob]) {
    let failed = failures(jobs);
    let again: Vec<Failure> = {
        let mut state = core.boards.state.locked();
        let board = state
            .boards
            .entry(project.to_owned())
            .or_insert_with(|| Board::empty(project));
        if board.ci_failing.sha == sha {
            return;
        }
        let before = std::mem::replace(
            &mut board.ci_failing,
            Latest {
                sha: sha.to_owned(),
                failures: failed.clone(),
            },
        );
        failed
            .into_iter()
            .filter(|failure| before.failures.iter().any(|was| was.same(failure)))
            .collect()
    };
    if again.is_empty() {
        return;
    }
    let output = jobs
        .iter()
        .filter(|job| again.iter().any(|failure| failure.check == job.check))
        .map(|job| {
            let mut said = job.check.clone();
            if let Some(url) = &job.url {
                said = format!("{said} ({url})");
            }
            match &job.error {
                Some(error) => format!("{said}\n{error}"),
                None => said,
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let broke = Broke {
        output: clipped(&output, MAX_OUTPUT_CHARS),
        sha: sha.to_owned(),
        failures: again,
    };
    let more = match broke.failures.len() {
        1 => String::new(),
        n => format!(" and {} more", n - 1),
    };
    let title = format!("CI keeps failing: {}{more}", broke.failures[0].label());
    let adding = Adding {
        title: clipped(&title, MAX_TITLE_CHARS),
        details: format!(
            "CI failed on the last two runs of the project's branch, most recently on {}, with {}. Mend it with a commit; Built passes once CI passes on it.",
            short(sha),
            labels(&broke.failures)
        ),
        promise: Some(PROMISE.to_owned()),
        ci: Some(broke),
        ..super::Adding::default()
    };
    match add_card(core, project, &By::Auto("CI"), adding, None) {
        Ok(_) => tell_clients(core),
        Err(error) => eprintln!("sikemux core: CI failing on {project} made no card: {error}"),
    }
}
