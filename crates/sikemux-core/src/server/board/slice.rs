//! A card that is a slice of a goal: the promise it makes, its ladder of
//! gates, and the evidence the core collected toward them. One table says who
//! passes each gate and what it needs first. An agent saying its slice is
//! done passes nothing by itself; only what the core collected can, checked
//! against git as it is then. The board follows the project's commits,
//! landings and checks as repository events, never by being called from git,
//! and when it may have missed one it holds back rather than pass.

mod follow;
mod ladder;
mod rollup;
mod shots;
mod words;

pub(crate) use follow::*;
pub(crate) use ladder::*;
pub(crate) use rollup::*;
pub(crate) use shots::*;
pub(crate) use words::*;

use std::collections::{BTreeSet, VecDeque};
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::inbox::InboxRecording;
use sikemux_view::slice::{
    Asserted, Assertion, Collect, Evidence, Gate, GateState, GateView, Need, NextGate, Note,
    Passer, Proof, SliceState, SliceView, Verdict, Wait,
};

use super::auto::{BoardSettings, Landing};
use super::environments::{ladder_steps, Passes, Step};
use super::{
    change_card, messages, project_at, project_view, send_notices, Board, By, Card, CardStatus,
    Changes, Checker, NoCheck, Notice, ProjectBoard, State,
};
use crate::git::Git;
use crate::server::connection::blocking;
use crate::server::edits::footer;
use crate::server::repo_events::{RepoEvent, Told};
use crate::server::Core;
use crate::tasks::config::ProjectCheck;

/// Evidence kept besides the pieces notes are pinned to; the oldest goes first.
const MAX_EVIDENCE: usize = 24;
const MAX_NOTES: usize = 32;
pub(super) const MAX_PROMISE_CHARS: usize = 200;
const MAX_NOTE_CHARS: usize = 4000;
const DIFF_FILES: usize = 8;
/// Check runs kept per project for commits the board hears of after them.
const MAX_HEARD_RUNS: usize = 32;
/// Times in a row the board tries to follow events it failed on, before it
/// lets them go.
const MAX_PANICS: u32 = 3;
/// Commits read back from the branch's tip for an agent's latest one.
const OWNER_COMMITS_READ: usize = 500;
/// Commits whose footer is kept as read, past which the board reads afresh.
const MAX_FOOTERS: usize = 4096;

pub(super) fn promise_of(text: &str) -> Result<String, String> {
    let promise = text.trim();
    if promise.is_empty() || promise.chars().count() > MAX_PROMISE_CHARS || promise.contains('\n') {
        return Err(format!(
            "promise must be one sentence of at most {MAX_PROMISE_CHARS} characters saying what the card changes"
        ));
    }
    Ok(promise.to_owned())
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum PassedBy {
    Agent(String),
    Person,
    /// The core, on what it collected, or Auto or a ticket's tracker for the person.
    Core,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pass {
    gate: Step,
    by: PassedBy,
    at: u64,
}

/// The owner's latest run of commits for the slice, with nobody else's
/// between them.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Work {
    /// The commit the run starts on, not its own.
    base: String,
    head: String,
    /// On the project's branch, rather than only in the owner's own checkout,
    /// as the commit, the landing or git said when `head` was last set.
    on_branch: bool,
    check: Option<Check>,
    /// When `head` became its latest work.
    #[serde(default)]
    at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Check {
    /// The gate the slice waited at when it ran.
    gate: Step,
    /// None when the check did not run.
    verdict: Option<Verdict>,
    /// The end of what it printed, or why it did not run.
    output: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Slice {
    pub promise: String,
    /// The steps it passes, in order.
    ladder: Vec<Step>,
    #[serde(default)]
    passes: Vec<Pass>,
    #[serde(default)]
    evidence: Vec<Evidence>,
    #[serde(default)]
    next_evidence: u64,
    #[serde(default)]
    notes: Vec<Note>,
    #[serde(default)]
    work: Option<Work>,
    /// Its owner said it is done since its latest work, so the owner's gate
    /// passes as soon as what it needs is so.
    #[serde(default)]
    pub(super) done_said: bool,
    /// The goal's standing assertions it fulfils, with the evidence each needs.
    #[serde(default)]
    asserts: Vec<Named>,
    /// Assertions it named that no longer stand on its goal, so no longer bind it.
    #[serde(default)]
    detached: Vec<Named>,
    /// Someone claimed it once: from then on only the person frees it of an assertion.
    #[serde(default)]
    pub started: bool,
    /// When someone last claimed it.
    #[serde(default)]
    pub started_at: Option<u64>,
    /// The card under it where a failed check is being fixed, while it is open.
    #[serde(default)]
    pub fix: Option<u64>,
    /// The owner it lost, until someone takes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stalled: Option<super::lease::Stalled>,
    /// Its work passed Built on the project's branch once, so the cards that wait on it go
    /// ahead; what it needs after that, such as a look, is new work they need not wait for.
    #[serde(default)]
    pub(super) built_in: bool,
}

/// An assertion of its goal a slice fulfils.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Named {
    /// The goal card whose assertion it is, so it binds no slice under another.
    pub goal: u64,
    pub id: u64,
    pub evidence: Collect,
    /// The person approved the command it names, which is run only then.
    #[serde(default)]
    pub approved: bool,
    /// The commit of the slice where a file that command names waits for the person to accept it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held_at: Option<String>,
    /// The commit of the slice where a file that command names is as the person turned it down.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused_at: Option<String>,
}

impl Named {
    /// The assertion `assertion` of goal `goal`, as slice `card` fulfils it.
    pub(super) fn of(goal: u64, assertion: &Assertion, card: u64) -> Self {
        Self {
            goal,
            id: assertion.id,
            evidence: assertion.evidence.clone(),
            approved: assertion.awaits_approval().is_none(),
            held_at: (assertion.files.changed.iter())
                .find(|changed| changed.card == card)
                .map(|changed| changed.head.clone()),
            refused_at: (assertion.files.refused.iter())
                .find(|refused| refused.card == card)
                .map(|refused| refused.head.clone()),
        }
    }
}

/// Why the owner's gate did not pass when the owner said the slice is done.
pub(super) struct Refused {
    pub text: String,
    /// The project's check failed on its latest commit, which a fix card mends.
    pub failed: Option<String>,
}

impl From<String> for Refused {
    fn from(text: String) -> Self {
        Self { text, failed: None }
    }
}

/// The owner's latest commit as git has it, made on `from`.
#[derive(Clone)]
pub(super) struct Head {
    pub from: String,
    pub head: String,
    pub on_branch: bool,
}

/// How the owner saying its slice is done went, when it was not refused.
pub(super) enum Said {
    Ready,
    /// Its work made no commit, so its owner closes it on the result it gave.
    Closed,
    /// Its gate passes by itself later; `recheck` is a commit to check again.
    Pending {
        note: String,
        recheck: Option<String>,
    },
}

/// A run of the project's check, kept for a commit heard of after it.
pub(super) struct HeardRun {
    sha: Option<String>,
    covered: Vec<String>,
    verdict: Option<Verdict>,
    command: String,
    output: String,
}

/// The latest check runs of one project, newest last.
#[derive(Default)]
pub(super) struct HeardRuns(VecDeque<HeardRun>);

impl HeardRun {
    /// A check run on `head` alone.
    pub(super) fn of(head: &str, command: &str, verdict: Verdict, output: &str) -> Self {
        Self {
            sha: Some(head.to_owned()),
            covered: vec![head.to_owned()],
            verdict: Some(verdict),
            command: command.to_owned(),
            output: output.to_owned(),
        }
    }
}

impl HeardRuns {
    fn keep(&mut self, run: HeardRun) {
        if self.0.len() >= MAX_HEARD_RUNS {
            self.0.pop_front();
        }
        self.0.push_back(run);
    }

    fn of(&self, sha: &str) -> Option<&HeardRun> {
        self.0
            .iter()
            .rev()
            .find(|run| run.covered.iter().any(|covered| covered == sha))
    }
}

impl Slice {
    /// Its owner sets out on it.
    pub(super) fn begin(&mut self, at: u64) {
        self.started = true;
        self.started_at = Some(at);
        self.stalled = None;
    }

    /// A new slice, keeping `recordings` the card had as pictures toward its
    /// first gate.
    pub(super) fn new(
        promise: String,
        recordings: Vec<InboxRecording>,
        owner: Option<&str>,
        settings: &BoardSettings,
    ) -> Self {
        let mut slice = Self {
            promise,
            ladder: ladder_of(settings, false),
            passes: Vec::new(),
            evidence: Vec::new(),
            next_evidence: 1,
            notes: Vec::new(),
            work: None,
            done_said: false,
            asserts: Vec::new(),
            detached: Vec::new(),
            started: false,
            started_at: None,
            fix: None,
            stalled: None,
            built_in: false,
        };
        for recording in &recordings {
            slice.picture(owner, recording);
        }
        slice
    }

    fn pass_of(&self, gate: &Step) -> Option<&Pass> {
        self.passes.iter().find(|pass| pass.gate == *gate)
    }

    pub(super) fn waiting(&self) -> Option<&Step> {
        self.ladder.iter().find(|gate| self.pass_of(gate).is_none())
    }

    /// The gate it waits at when that is the person's to pass and nothing it
    /// needs is missing.
    pub(super) fn waits_on_person(&self, settings: &BoardSettings) -> Option<&Step> {
        self.waiting().filter(|gate| {
            passer(gate, settings) == Passer::Person && self.unmet(gate, settings).is_empty()
        })
    }

    pub(super) fn head(&self) -> Option<&str> {
        self.work.as_ref().map(|work| work.head.as_str())
    }

    /// Since when it has waited at its gate: the gate before it passed, or
    /// its latest work came, whichever is later.
    pub(super) fn waiting_since(&self) -> u64 {
        let passed = self.passes.iter().map(|pass| pass.at).max().unwrap_or(0);
        let worked = self.work.as_ref().map_or(0, |work| work.at);
        passed.max(worked)
    }

    /// The commit whose check the gate it waits at still needs, once the
    /// check has not said anything of it yet.
    pub(super) fn awaits_check(&self, settings: &BoardSettings) -> Option<&str> {
        let gate = self.waiting()?;
        let work = self.work.as_ref()?;
        (needs_of(gate, settings).contains(&Need::PassingCheck) && work.check.is_none())
            .then_some(work.head.as_str())
    }

    /// How long passing `gate` took, for a slice that passed it: from the claim
    /// for its first gate, from the gate before for the rest. None when several
    /// passed at once, as closing a slice does, which says nothing of the work.
    pub(super) fn took(&self, gate: &Step) -> Option<(u64, u64)> {
        let at = self.pass_of(gate)?.at;
        let place = self.ladder.iter().position(|known| known == gate)?;
        let from = match place.checked_sub(1) {
            None => self.started_at?,
            Some(before) => self.pass_of(&self.ladder[before])?.at,
        };
        (at > from).then(|| (at, at - from))
    }

    /// The gates after `gate` until the next only the person passes, and whether
    /// there is one.
    pub(super) fn ahead(&self, gate: &Step, settings: &BoardSettings) -> (Vec<&Step>, bool) {
        let mut gates = Vec::new();
        for next in self
            .ladder
            .iter()
            .skip_while(|known| *known != gate)
            .skip(1)
        {
            if passer(next, settings) == Passer::Person {
                return (gates, true);
            }
            gates.push(next);
        }
        (gates, false)
    }

    /// When someone claimed it, or the gate before `gate` passed, as work on
    /// that gate began.
    pub(super) fn working_on_since(&self, gate: &Step) -> Option<u64> {
        match self.ladder.first() == Some(gate) {
            true => self.started_at,
            false => Some(self.waiting_since()),
        }
    }

    /// A check toward the gate it waits at failed since `since`, and whether
    /// its latest commit's check is on its way.
    pub(super) fn failed_since(&self, since: u64, settings: &BoardSettings) -> Option<bool> {
        let gate = self.waiting()?;
        let latest = (self.evidence.iter().rev())
            .find(|evidence| toward(evidence) == *gate && evidence.verdict.is_some())?;
        (latest.verdict == Some(Verdict::Failed) && latest.at >= since)
            .then(|| self.awaits_check(settings).is_some())
    }

    /// Its latest commit came after `since`, replacing one the check had a say on.
    pub(super) fn reworked_since(&self, since: u64) -> bool {
        self.work.as_ref().is_some_and(|work| {
            work.at > since
                && (self.evidence.iter()).any(|evidence| {
                    evidence.verdict.is_some() && evidence.commit.as_ref() != Some(&work.head)
                })
        })
    }

    /// Its work is on the project's branch for good: its Landed gate passed,
    /// or on a ladder with no such gate, its first environment's needs were met.
    pub(super) fn landed(&self) -> bool {
        let landed = Step::Gate(Gate::Landed);
        match self.ladder.contains(&landed) {
            true => self.pass_of(&landed).is_some(),
            false => {
                self.pass_of(&Step::Gate(Gate::Built)).is_some()
                    && self.work.as_ref().is_some_and(|work| work.on_branch)
            }
        }
    }

    /// Notes when its work has passed Built on the project's branch.
    fn note_built_in(&mut self) {
        self.built_in |= self.pass_of(&Step::Gate(Gate::Built)).is_some()
            && self.work.as_ref().is_some_and(|work| work.on_branch);
    }

    fn evidence(&self, id: u64) -> Option<&Evidence> {
        self.evidence.iter().find(|evidence| evidence.id == id)
    }

    /// The evidence for the gate waiting, a picture first, the newest of each; the newest of all
    /// once nothing is toward that gate.
    fn leading(&self) -> Option<&Evidence> {
        let gate = self.waiting();
        let at_gate = || {
            (self.evidence.iter())
                .filter(move |evidence| gate.is_none_or(|gate| toward(evidence) == *gate))
        };
        at_gate()
            .filter(|evidence| matches!(evidence.proof, Proof::Picture { .. }))
            .max_by_key(|evidence| evidence.at)
            .or_else(|| at_gate().max_by_key(|evidence| evidence.at))
            .or_else(|| self.evidence.iter().max_by_key(|evidence| evidence.at))
    }

    /// The evidence its gate leads with, as much as a thumbnail of it needs.
    pub(super) fn lead_in_short(&self) -> Option<Evidence> {
        let mut lead = self.leading()?.clone();
        if let Proof::Run { output, .. } = &mut lead.proof {
            output.clear();
        }
        Some(lead)
    }

    fn met(&self, need: Need) -> bool {
        let Some(work) = &self.work else {
            return need == Need::Assertions && self.asserts.is_empty();
        };
        match need {
            Need::PassingCheck => work
                .check
                .as_ref()
                .is_some_and(|check| check.verdict == Some(Verdict::Passed)),
            Need::OnBranch => work.on_branch,
            Need::Assertions => self.asserts.iter().all(|named| self.asserted(named)),
        }
    }

    pub(super) fn asserts(&self) -> impl Iterator<Item = u64> + '_ {
        self.asserts.iter().map(|named| named.id)
    }

    pub(super) fn asserting(&self, kind: Collect) -> Vec<u64> {
        (self.asserts.iter())
            .filter(|named| named.evidence == kind)
            .map(|named| named.id)
            .collect()
    }

    fn latest_for(&self, named: &Named) -> Option<&Evidence> {
        (self.evidence.iter().rev()).find(|evidence| {
            evidence.assertions.contains(&named.id)
                && match (&evidence.proof, &named.evidence) {
                    (Proof::Picture { seconds, .. }, Collect::Recording) => seconds.is_some(),
                    (Proof::Picture { seconds, .. }, Collect::Screenshot) => seconds.is_none(),
                    (Proof::Run { .. }, Collect::Workflow { .. }) => true,
                    (Proof::Run { command: ran, .. }, Collect::Command { command }) => {
                        ran == command
                    }
                    _ => false,
                }
        })
    }

    /// The assertion holds on its latest commit: the check's verdict on it
    /// passed, for one the check shows; its workflow's run passed there, for
    /// one a workflow shows; the approved command exited with success there,
    /// for one a command shows; else a recording or screenshot was collected
    /// for it there, which the person judges at Looks right.
    fn asserted(&self, named: &Named) -> bool {
        let on_head = self
            .latest_for(named)
            .filter(|evidence| evidence.commit.as_deref() == self.head());
        match named.evidence {
            Collect::Check => self.met(Need::PassingCheck),
            Collect::Workflow { .. } => {
                on_head.is_some_and(|evidence| evidence.verdict == Some(Verdict::Passed))
            }
            Collect::Command { .. } => {
                !self.unapproved(named)
                    && on_head.is_some_and(|evidence| evidence.verdict == Some(Verdict::Passed))
            }
            Collect::Recording | Collect::Screenshot => on_head.is_some(),
        }
    }

    /// The workflows whose run has not passed on its latest commit, so a
    /// first run or a re-run is still awaited: each assertion's id and workflow.
    pub(super) fn awaiting_workflows(&self) -> Vec<(u64, String)> {
        (self.asserts.iter())
            .filter(|named| !self.asserted(named))
            .filter_map(|named| match &named.evidence {
                Collect::Workflow { name } => Some((named.id, name.clone())),
                _ => None,
            })
            .collect()
    }

    /// The approved commands that have not passed on its latest commit.
    pub(super) fn awaiting_commands(&self) -> Vec<String> {
        let mut commands: Vec<String> = (self.asserts.iter())
            .filter(|named| !self.unapproved(named) && !self.asserted(named))
            .filter_map(|named| match &named.evidence {
                Collect::Command { command } => Some(command.clone()),
                _ => None,
            })
            .collect();
        commands.sort();
        commands.dedup();
        commands
    }

    /// `command` ran on `head`, with no verdict when it did not finish: the run is the evidence of
    /// each assertion that approved command shows, kept only while `head` is still its latest
    /// commit. Those assertions.
    pub(super) fn command_ran(
        &mut self,
        owner: Option<&str>,
        head: &str,
        command: &str,
        verdict: Option<Verdict>,
        output: String,
    ) -> Vec<u64> {
        let Some(gate) = self
            .waiting()
            .filter(|_| self.head() == Some(head))
            .cloned()
        else {
            return Vec::new();
        };
        let shows = Collect::Command {
            command: command.to_owned(),
        };
        let assertions: Vec<u64> = (self.asserts.iter())
            .filter(|named| !self.unapproved(named) && named.evidence == shows)
            .map(|named| named.id)
            .collect();
        if !assertions.is_empty() {
            let proof = Proof::Run {
                command: command.to_owned(),
                sha: Some(head.to_owned()),
                output,
            };
            let owner = owner.map(str::to_owned);
            self.add_evidence(&gate, epoch_ms(), owner, verdict, proof, &assertions);
        }
        assertions
    }

    /// Its gate needs assertions whose commands wait for the person's approval.
    pub(super) fn awaits_approval(&self, settings: &BoardSettings) -> bool {
        self.gate_needs_assertions(settings)
            && self.asserts.iter().any(|named| self.unapproved(named))
    }

    /// Its command waits for the person: the command itself, or a file it names on its latest
    /// commit.
    fn unapproved(&self, named: &Named) -> bool {
        matches!(named.evidence, Collect::Command { .. })
            && (!named.approved
                || named.held_at.is_some() && named.held_at.as_deref() == self.head())
    }

    /// The assertions it fulfils that `command` shows, each with the goal that keeps it.
    pub(super) fn commanded_by(&self, command: &str) -> Vec<(u64, u64)> {
        (self.asserts.iter())
            .filter(|named| {
                named.approved && matches!(&named.evidence, Collect::Command { command: wanted } if wanted == command)
            })
            .map(|named| (named.goal, named.id))
            .collect()
    }

    fn gate_needs_assertions(&self, settings: &BoardSettings) -> bool {
        self.waiting()
            .is_some_and(|gate| needs_of(gate, settings).contains(&Need::Assertions))
    }

    /// An assertion's approved command failed on its latest commit, at a gate that needs it.
    fn command_failed(&self, settings: &BoardSettings) -> bool {
        self.gate_needs_assertions(settings)
            && (self.asserts.iter())
                .filter(|named| {
                    matches!(named.evidence, Collect::Command { .. }) && !self.unapproved(named)
                })
                .filter_map(|named| self.latest_for(named))
                .any(|evidence| {
                    evidence.verdict == Some(Verdict::Failed)
                        && evidence.commit.as_deref() == self.head()
                })
    }

    /// Workflow `name` ruled on `head`, held by the pushed `tip`: the run is
    /// the evidence of each of its assertions that workflow shows, with no
    /// verdict when it gave none. Kept only while `head` is still its latest
    /// commit, and once for the same ruling.
    pub(super) fn workflow_ran(
        &mut self,
        head: &str,
        name: &str,
        tip: &str,
        verdict: Option<Verdict>,
        output: String,
    ) {
        let Some(gate) = self
            .waiting()
            .filter(|_| self.head() == Some(head))
            .cloned()
        else {
            return;
        };
        let proof = Proof::Run {
            command: name.to_owned(),
            sha: Some(tip.to_owned()),
            output,
        };
        let shows = Collect::Workflow {
            name: name.to_owned(),
        };
        let ruled_already = (self.asserts.iter())
            .filter(|named| named.evidence == shows)
            .all(|named| {
                self.latest_for(named).is_some_and(|evidence| {
                    evidence.commit.as_deref() == Some(head)
                        && evidence.verdict == verdict
                        && evidence.proof == proof
                })
            });
        if ruled_already {
            return;
        }
        let assertions = self.asserting(shows);
        self.add_evidence(&gate, epoch_ms(), None, verdict, proof, &assertions);
    }

    /// A run that failed before its work began, such as CI's on the
    /// project's branch, kept as what the work is to mend.
    pub(super) fn failed_run(&mut self, command: String, sha: &str, output: String) {
        let Some(gate) = self.waiting().cloned() else {
            return;
        };
        let proof = Proof::Run {
            command,
            sha: Some(sha.to_owned()),
            output,
        };
        self.add_evidence(&gate, epoch_ms(), None, Some(Verdict::Failed), proof, &[]);
    }

    /// The assertions still without what they need on its latest commit.
    fn missing(&self) -> Vec<String> {
        (self.asserts.iter())
            .filter(|named| !self.asserted(named))
            .map(|named| format!("#{}", named.id))
            .collect()
    }

    /// Names the goal's assertions it fulfils, which stand on its goal.
    pub(super) fn name(&mut self, named: Vec<Named>) {
        (self.detached).retain(|kept| !named.iter().any(|named| named.id == kept.id));
        self.asserts = named;
    }

    /// Whether it is bound to an assertion.
    pub(super) fn bound(&self) -> bool {
        !self.asserts.is_empty()
    }

    /// Keeps it bound to the assertions of goal `goal` it names that still
    /// stand there, as `standing` says each stands now; one that no longer
    /// stands is detached. Those of another goal, which it moved from, it is
    /// freed of. Whether it was.
    pub(super) fn bind(&mut self, goal: u64, standing: impl Fn(u64) -> Option<Named>) -> bool {
        let moved = self.asserts.iter().any(|named| named.goal != goal);
        self.detached.retain(|named| named.goal == goal);
        for named in std::mem::take(&mut self.asserts) {
            match (named.goal == goal).then(|| standing(named.id)).flatten() {
                Some(standing) => self.asserts.push(standing),
                None if named.goal == goal && !self.detached.contains(&named) => {
                    self.detached.push(named)
                }
                None => {}
            }
        }
        moved
    }

    fn unmet(&self, gate: &Step, settings: &BoardSettings) -> Vec<Need> {
        needs_of(gate, settings)
            .into_iter()
            .filter(|need| !self.met(*need))
            .collect()
    }

    /// Keeps a piece of evidence on its latest commit, collected for
    /// `assertions`, letting the oldest go that no note is pinned to and is
    /// not an assertion's latest.
    fn add_evidence(
        &mut self,
        gate: &Step,
        at: u64,
        agent: Option<String>,
        verdict: Option<Verdict>,
        proof: Proof,
        assertions: &[u64],
    ) {
        let latest: Vec<u64> = (self.asserts.iter())
            .filter_map(|named| self.latest_for(named).map(|evidence| evidence.id))
            .collect();
        let notes = &self.notes;
        let pinned = |evidence: &Evidence| {
            latest.contains(&evidence.id)
                || notes.iter().any(|note| note.evidence == Some(evidence.id))
        };
        let loose: Vec<usize> = (0..self.evidence.len())
            .filter(|at| !pinned(&self.evidence[*at]))
            .collect();
        if loose.len() >= MAX_EVIDENCE {
            self.evidence.remove(loose[0]);
        }
        let id = self.next_evidence.max(1);
        self.next_evidence = id + 1;
        self.evidence.push(Evidence {
            id,
            gate: gate.gate(),
            environment: gate.environment().map(str::to_owned),
            at,
            agent,
            verdict,
            proof,
            commit: self.head().map(str::to_owned),
            assertions: assertions.to_vec(),
        });
    }

    /// Keeps a recording toward the gate it waits at, for the assertions a
    /// recording shows. False when it waits at none, being done, so the
    /// recording is not kept.
    fn picture(&mut self, agent: Option<&str>, recording: &InboxRecording) -> bool {
        let Some(gate) = self.waiting().cloned() else {
            return false;
        };
        let assertions = self.asserting(Collect::Recording);
        self.add_evidence(
            &gate,
            recording.at,
            agent.map(str::to_owned),
            None,
            Proof::Picture {
                path: recording.path.clone(),
                seconds: Some(recording.seconds),
                feed: recording.feed.clone(),
                turn: recording.turn,
                width: None,
                opened: None,
            },
            &assertions,
        );
        true
    }

    /// Keeps the screenshots at `shots`, the agent's and those the core took
    /// beside it at set widths, for the assertions a screenshot shows, in place
    /// of those they had, unless a note is pinned to one.
    fn screenshots(&mut self, agent: &str, shots: &[Shot]) {
        let Some(gate) = self.waiting().cloned() else {
            return;
        };
        let assertions = self.asserting(Collect::Screenshot);
        let notes = &self.notes;
        self.evidence.retain(|evidence| {
            let replaced = matches!(evidence.proof, Proof::Picture { seconds: None, .. })
                && evidence.assertions.iter().any(|id| assertions.contains(id));
            !replaced || notes.iter().any(|note| note.evidence == Some(evidence.id))
        });
        for (path, width) in shots {
            self.add_evidence(
                &gate,
                epoch_ms(),
                Some(agent.to_owned()),
                None,
                Proof::Picture {
                    path: path.clone(),
                    seconds: None,
                    feed: None,
                    turn: None,
                    width: *width,
                    opened: None,
                },
                &assertions,
            );
        }
    }

    /// Keeps the screenshots the core took itself of the page at `opened`,
    /// which the change ending at `head` shows, toward the gate it waits at.
    /// They show the running thing, but no assertion's evidence.
    pub(super) fn page(&mut self, owner: &str, head: &str, opened: &str, shots: &[Shot]) {
        let Some(gate) = self
            .waiting()
            .filter(|_| self.head() == Some(head))
            .cloned()
        else {
            return;
        };
        for (path, width) in shots {
            self.add_evidence(
                &gate,
                epoch_ms(),
                Some(owner.to_owned()),
                None,
                Proof::Picture {
                    path: path.clone(),
                    seconds: None,
                    feed: None,
                    turn: None,
                    width: *width,
                    opened: Some(opened.to_owned()),
                },
                &[],
            );
        }
    }

    /// The owner's work now ends at `head`, made on `from`, and takes the
    /// verdict of a check run heard before it. New work undoes every gate
    /// from the first that needs the check on it; work made again elsewhere,
    /// as landing does, keeps the gates it passed.
    fn worked(&mut self, at: &Head, new_work: bool, owner: Option<&str>, runs: Option<&HeardRuns>) {
        if let Some(work) = self.work.as_mut().filter(|work| work.head == at.head) {
            work.on_branch |= at.on_branch;
            return;
        }
        let base = match &self.work {
            Some(work) if work.head == at.from => work.base.clone(),
            _ => at.from.clone(),
        };
        self.work = Some(Work {
            base,
            head: at.head.clone(),
            on_branch: at.on_branch,
            check: None,
            at: epoch_ms(),
        });
        if new_work {
            self.back_to_work();
        }
        if let Some(run) = runs.and_then(|runs| runs.of(&at.head)) {
            self.checked(owner, run);
        }
    }

    /// Undoes every gate from the first that needs the check, until its owner says it is done again.
    fn back_to_work(&mut self) {
        self.done_said = false;
        if let Some(from) = self
            .ladder
            .iter()
            .position(|gate| rule(gate).needs.contains(&Need::PassingCheck))
        {
            let undone = &self.ladder[from..];
            self.passes.retain(|pass| !undone.contains(&pass.gate));
        }
    }

    /// A run of the project's check ended. Whether it took in this slice's
    /// work, which then holds the run as evidence and its verdict.
    fn checked(&mut self, owner: Option<&str>, run: &HeardRun) -> bool {
        let (Some(gate), Some(head)) = (self.waiting().cloned(), self.head()) else {
            return false;
        };
        if !run.covered.iter().any(|covered| covered == head) {
            return false;
        }
        self.add_evidence(
            &gate,
            epoch_ms(),
            owner.map(str::to_owned),
            run.verdict,
            Proof::Run {
                command: run.command.clone(),
                sha: run.sha.clone(),
                output: run.output.clone(),
            },
            &[],
        );
        if let Some(work) = &mut self.work {
            work.check = Some(Check {
                gate,
                verdict: run.verdict,
                output: run.output.clone(),
            });
        }
        true
    }

    /// The owner said it is done and the check did not pass: what it is told,
    /// which ends that it said so, and the commit it failed on.
    fn refused(&mut self, label: &str) -> Option<Refused> {
        if !self.done_said {
            return None;
        }
        let gate = self.waiting()?.clone();
        let work = self.work.as_ref()?;
        let check = work.check.as_ref()?;
        let refused = match check.verdict {
            Some(Verdict::Passed) => return None,
            Some(Verdict::Failed) => Refused {
                text: format!(
                    "Your card {label} did not pass {}: the project's check failed on {}.",
                    gate_words(&gate),
                    short(&work.head)
                ),
                failed: Some(work.head.clone()),
            },
            None => Refused::from(format!(
                "Your card {label} did not pass {}: the project's check did not run on {} ({}). Mark it done again to have it checked.",
                gate_words(&gate),
                short(&work.head),
                check.output
            )),
        };
        self.done_said = false;
        Some(refused)
    }

    /// Passes each gate in turn whose passer may pass it now and whose needs
    /// are met: the owner's once it said so, which git confirmed. The gates passed.
    fn advance(&mut self, owner: Option<&str>, settings: &BoardSettings) -> Vec<Step> {
        let mut passed = Vec::new();
        while let Some(gate) = self.waiting().cloned() {
            if !self.unmet(&gate, settings).is_empty() {
                break;
            }
            let by = match (passer(&gate, settings), owner) {
                (Passer::Core, _) if gate.environment().is_some() && !self.went_to(&gate) => break,
                (Passer::Core, _) => PassedBy::Core,
                (Passer::Owner, Some(owner)) if self.done_said => PassedBy::Agent(owner.to_owned()),
                _ => break,
            };
            self.passes.push(Pass {
                gate: gate.clone(),
                by,
                at: epoch_ms(),
            });
            passed.push(gate);
        }
        passed
    }

    /// The latest deploy to the environment `step` is, on its latest commit,
    /// did not fail: one the core saw end well, or one it could not follow.
    fn went_to(&self, step: &Step) -> bool {
        (self.evidence.iter().rev())
            .find(|evidence| evidence.environment.is_some() && toward(evidence) == *step)
            .is_some_and(|evidence| {
                evidence.verdict != Some(Verdict::Failed)
                    && evidence.commit.as_deref() == self.head()
            })
    }

    fn deploy_failed(&self, step: &Step) -> bool {
        step.environment().is_some()
            && (self.evidence.iter().rev())
                .find(|evidence| evidence.environment.is_some() && toward(evidence) == *step)
                .is_some_and(|evidence| evidence.verdict == Some(Verdict::Failed))
    }

    /// Keeps a deploy to `environment` as evidence toward its step. One that
    /// failed undoes that step and those after it, when `undoes`.
    pub(super) fn deploy_ran(
        &mut self,
        environment: &str,
        agent: Option<&str>,
        verdict: Option<Verdict>,
        proof: Proof,
        undoes: bool,
    ) {
        let step = Step::Environment(environment.to_owned());
        self.add_evidence(
            &step,
            epoch_ms(),
            agent.map(str::to_owned),
            verdict,
            proof,
            &[],
        );
        if undoes && verdict == Some(Verdict::Failed) {
            if let Some(from) = self.ladder.iter().position(|known| *known == step) {
                let undone = &self.ladder[from..];
                self.passes.retain(|pass| !undone.contains(&pass.gate));
            }
        }
    }

    /// Takes the ladder its project gives it now, keeping the passes of the
    /// steps that stay.
    fn take_ladder(&mut self, ladder: Vec<Step>) {
        self.passes.retain(|pass| ladder.contains(&pass.gate));
        self.ladder = ladder;
    }

    /// Someone closes the slice: every gate left passes as `by`'s. The gates passed.
    pub(super) fn close(&mut self, by: PassedBy) -> Vec<Step> {
        let mut passed = Vec::new();
        while let Some(gate) = self.waiting().cloned() {
            self.passes.push(Pass {
                gate: gate.clone(),
                by: by.clone(),
                at: epoch_ms(),
            });
            passed.push(gate);
        }
        passed
    }

    /// Its pull request merged, so it landed: the gates left before any
    /// environment pass, the person's and its owner's as the person's when they
    /// merged it themselves, the rest as the core's. What a deploy passes still
    /// waits for one. Whether nothing is left.
    pub(super) fn merged(&mut self, by_person: bool, settings: &BoardSettings) -> bool {
        if let Some(work) = &mut self.work {
            work.on_branch = true;
        }
        while let Some(gate) = self
            .waiting()
            .filter(|gate| gate.environment().is_none())
            .cloned()
        {
            let by = match by_person && passer(&gate, settings) != Passer::Core {
                true => PassedBy::Person,
                false => PassedBy::Core,
            };
            self.passes.push(Pass {
                gate,
                by,
                at: epoch_ms(),
            });
        }
        self.note_built_in();
        self.waiting().is_none()
    }

    /// It is back at work, so the gates it passed go.
    pub(super) fn restart(&mut self) {
        self.passes.clear();
        self.done_said = false;
        self.stalled = None;
    }

    /// Nobody holds it any more, so its work goes with the gates it passed.
    pub(super) fn reopen(&mut self) {
        self.restart();
        self.work = None;
        self.built_in = false;
    }

    /// The person passes `gate`, when it is the one waiting and theirs.
    pub(super) fn pass_by_person(
        &mut self,
        gate: &Step,
        label: &str,
        settings: &BoardSettings,
    ) -> Result<(), String> {
        match self.waiting() {
            Some(waiting) if waiting != gate => {
                return Err(format!(
                    "{label} waits at {}, not {}",
                    gate_words(waiting),
                    gate_words(gate)
                ))
            }
            None => return Err(format!("{label} passed every gate already")),
            Some(_) => {}
        }
        let theirs = gate.environment().is_some() && self.unmet(gate, settings).is_empty()
            || self.waits_on_person(settings) == Some(gate);
        if !theirs {
            return Err(format!(
                "{} is not yours to pass on {label}",
                gate_words(gate)
            ));
        }
        self.passes.push(Pass {
            gate: gate.clone(),
            by: PassedBy::Person,
            at: epoch_ms(),
        });
        Ok(())
    }

    /// The owner says the slice is done, with `result` saying what it did. Refuses, changing
    /// nothing, when nothing the core collected can show its latest commit sound, or git does
    /// not confirm it is its latest: `confirmed`. Work that made no commit, such as a deploy,
    /// closes on its result, unless an assertion needs evidence on a commit.
    pub(super) fn say_done(
        &mut self,
        label: &str,
        checking: &Checking,
        confirmed: Result<(), &str>,
        result: Option<&str>,
        settings: &BoardSettings,
    ) -> Result<Said, Refused> {
        let Some(gate) = self.waiting().cloned() else {
            return Err(format!("{label} is done already").into());
        };
        if let Some(environment) = gate.environment() {
            return Err(format!(
                "{label} waits at {environment}, which a deploy there passes, not you. Deploy it; {}. If it needs more work, commit it and mark the card done again",
                settings.passes_words(environment)
            )
            .into());
        }
        if passer(&gate, settings) != Passer::Owner {
            return Err(format!(
                "{label} waits at {}, which is not yours to pass. If it needs more work, commit it and mark the card done again",
                gate_words(&gate)
            )
            .into());
        }
        let words = gate_words(&gate);
        let Some(work) = &self.work else {
            let said = result.is_some_and(|result| !result.trim().is_empty());
            return match (said, self.asserts.is_empty()) {
                (true, true) => Ok(Said::Closed),
                (_, false) => Err(format!(
                    "Not {words}: you have committed nothing for {label}, and the assertions it fulfils need evidence on a commit. Commit its work with git_commit, then mark it done once the project's check passes on it"
                )
                .into()),
                (false, true) => Err(format!(
                    "Not {words}: you have committed nothing for {label}. Commit its work with git_commit, then mark it done once the project's check passes on it. If its work makes no commit, such as a deploy, mark it done with result saying what you did and what showed it worked"
                )
                .into()),
            };
        };
        if let Err(why) = confirmed {
            return Err(format!("Not {words}: {why}").into());
        }
        let unmet = self.unmet(&gate, settings).contains(&Need::PassingCheck);
        let said = match (&work.check, unmet, checking) {
            (_, false, _) => Said::Ready,
            (Some(Check { verdict: Some(Verdict::Failed), output, .. }), true, _) => {
                return Err(Refused {
                    text: format!(
                        "Not {words}: the project's check failed on {}, your latest commit.\n\n{output}",
                        short(&work.head)
                    ),
                    failed: Some(work.head.clone()),
                });
            }
            (_, true, Checking::Nothing) => {
                return Err(format!(
                    "Not {words}: this project names no check in sikemux.json, neither a command nor CI, and Sikemux found no test command in its build files, so nothing can show the work sound. Ask your lead or the person how it is to be checked"
                )
                .into())
            }
            (_, true, Checking::Unread(why)) => {
                return Err(format!(
                    "Not {words}: {why}, so Sikemux can't read the check it names and nothing can show the work sound until it is fixed. Tell your lead or the person what is wrong with it"
                )
                .into())
            }
            (_, true, Checking::Ci { pushed: false }) => Said::Pending {
                note: format!(
                    "Not {words} yet: this project's check is its CI, which checks the project's branch as pushed, and {} isn't on it there yet. Land it if it isn't on the project's branch, then push the branch; {words} passes by itself once CI passes on it, and you will hear.",
                    short(&work.head)
                ),
                recheck: Some(work.head.clone()),
            },
            (_, true, Checking::Ci { pushed: true }) => Said::Pending {
                note: format!(
                    "Not {words} yet: CI has not passed on {}, your latest commit. {words} passes by itself once it does, and you will hear.",
                    short(&work.head)
                ),
                recheck: Some(work.head.clone()),
            },
            (_, true, Checking::Command) if work.on_branch => Said::Pending {
                note: format!(
                    "Not {words} yet: the project's check has not passed on {}, your latest commit. {words} passes by itself once it does, and you will hear.",
                    short(&work.head)
                ),
                recheck: Some(work.head.clone()),
            },
            (_, true, Checking::Command) => Said::Pending {
                note: format!(
                    "Not {words} yet: the project's check runs on the project's branch, and {} is only in your checkout. Land it with git_commit land true; {words} passes by itself once the check passes on it, and you will hear.",
                    short(&work.head)
                ),
                recheck: None,
            },
        };
        let said = match (said, self.awaits_evidence(label)) {
            (Said::Ready, Some(note)) => Said::Pending {
                note,
                recheck: None,
            },
            (said, _) => said,
        };
        self.done_said = true;
        Ok(said)
    }

    /// What its owner is told when the check vouches for its latest commit
    /// but some of its assertions still have no evidence there.
    fn awaits_evidence(&self, label: &str) -> Option<String> {
        let missing = self.missing();
        let head = self.head()?;
        if !self.met(Need::PassingCheck) || missing.is_empty() {
            return None;
        }
        if (self.asserts.iter()).any(|named| self.unapproved(named)) {
            return Some(format!(
                "{label} is not Built yet: a command one of its assertions names, or a file that command names, waits for the person to accept it, which only they can do, and the command does not run until they do. Built passes by itself once it has run and passed on {}, and you will hear. board_list with the goal's card shows which.",
                short(head)
            ));
        }
        Some(format!(
            "{label} is not Built yet: the check passed on {}, but {} no evidence there. Make what it asks for, a browser recording or a screenshot of the page, while this card is yours; Sikemux runs a command or reads a CI workflow itself. Built passes by itself once each has it, and you will hear. board_list with the goal's card shows what each asks.",
            short(head),
            match missing.as_slice() {
                [one] => format!("assertion {one} has"),
                many => format!("assertions {} have", many.join(", ")),
            }
        ))
    }

    /// The person writes on it, on a piece of its evidence when given. The
    /// agent whose work that evidence shows, and what the note is about.
    fn note(
        &mut self,
        text: String,
        evidence: Option<u64>,
    ) -> Result<(Option<String>, String), String> {
        let on = match evidence {
            Some(id) => Some(
                self.evidence(id)
                    .ok_or_else(|| format!("it has no evidence {id}"))?
                    .clone(),
            ),
            None => None,
        };
        if self.notes.len() >= MAX_NOTES {
            self.notes.remove(0);
        }
        self.notes.push(Note {
            text,
            by: None,
            evidence,
            at: epoch_ms(),
        });
        let about = match &on {
            Some(on) => format!(
                "the {} it holds toward {}",
                proof_words(&on.proof),
                gate_words(&toward(on))
            ),
            None => match self.waiting() {
                Some(gate) => format!("it, waiting at {}", gate_words(gate)),
                None => "it".to_owned(),
            },
        };
        Ok((on.and_then(|on| on.agent), about))
    }

    pub(super) fn screenshot_files(&self) -> impl Iterator<Item = &str> {
        self.evidence
            .iter()
            .filter_map(|evidence| match &evidence.proof {
                Proof::Picture {
                    path,
                    seconds: None,
                    ..
                } => Some(path.as_str()),
                _ => None,
            })
    }

    pub(super) fn pictures(&self) -> Vec<InboxRecording> {
        self.evidence
            .iter()
            .filter_map(|evidence| match &evidence.proof {
                Proof::Picture {
                    path,
                    seconds: Some(seconds),
                    feed,
                    turn,
                    ..
                } => Some(InboxRecording {
                    path: path.clone(),
                    seconds: *seconds,
                    at: evidence.at,
                    feed: feed.clone(),
                    turn: *turn,
                }),
                _ => None,
            })
            .collect()
    }

    /// `held_up`: it waits on another card, or nobody holds it.
    fn state(&self, held_up: bool, settings: &BoardSettings) -> SliceState {
        let Some(gate) = self.waiting() else {
            return SliceState::Done;
        };
        let failed = self
            .work
            .as_ref()
            .and_then(|work| work.check.as_ref())
            .is_some_and(|check| check.gate == *gate && check.verdict == Some(Verdict::Failed))
            || self.deploy_failed(gate)
            || self.command_failed(settings);
        if failed {
            SliceState::Failed
        } else if self.stalled.is_some()
            || self.waits_on_person(settings).is_some()
            || self.awaits_approval(settings)
        {
            SliceState::NeedsYou
        } else if held_up {
            SliceState::Waiting
        } else {
            SliceState::Moving
        }
    }

    pub(super) fn view(
        &self,
        owner: Option<&str>,
        held_up: bool,
        name: impl Fn(&str) -> String,
        wait: Option<Wait>,
        settings: &BoardSettings,
    ) -> SliceView {
        let waiting = self.waiting();
        let gates = self
            .ladder
            .iter()
            .map(|gate| {
                let passer = passer(gate, settings);
                let pass = self.pass_of(gate);
                let agent = match (pass.map(|pass| &pass.by), passer) {
                    (Some(PassedBy::Agent(agent)), _) => Some(agent.clone()),
                    (None, Passer::Owner) => owner.map(str::to_owned),
                    _ => None,
                };
                GateView {
                    gate: gate.gate(),
                    environment: gate.environment().map(str::to_owned),
                    why: gate
                        .environment()
                        .map(|name| settings.step_words(name).to_owned()),
                    passer,
                    state: match (pass, waiting == Some(gate)) {
                        (Some(_), _) => GateState::Passed,
                        (None, true) => GateState::Waiting,
                        (None, false) => GateState::Ahead,
                    },
                    agent_name: agent.as_deref().map(&name),
                    agent,
                    at: pass.map(|pass| pass.at),
                    needs: if pass.is_some() {
                        Vec::new()
                    } else {
                        self.unmet(gate, settings)
                    },
                }
            })
            .collect();
        SliceView {
            promise: self.promise.clone(),
            state: self.state(held_up, settings),
            gates,
            evidence: self.evidence.clone(),
            leading: self.leading().map(|evidence| evidence.id),
            notes: self.notes.clone(),
            asserts: (self.asserts.iter())
                .map(|named| Asserted {
                    assertion: named.id,
                    met: self.asserted(named),
                    awaits_approval: self.unapproved(named),
                    turned_down: named.refused_at.is_some()
                        && named.refused_at.as_deref() == self.head(),
                })
                .collect(),
            detached: self.detached.iter().map(|named| named.id).collect(),
            wait,
        }
    }

    /// What `board_list` tells an agent of where it stands.
    pub(super) fn list_into(&self, fields: &mut Map<String, Value>, settings: &BoardSettings) {
        fields.insert("promise".into(), self.promise.clone().into());
        if let Some(gate) = self.waiting() {
            fields.insert("waitsAt".into(), gate_key(gate).into());
            if let Some(environment) = gate.environment() {
                fields.insert("deployTo".into(), settings.passes_words(environment).into());
            }
            let needs: Vec<&str> = self
                .unmet(gate, settings)
                .into_iter()
                .map(need_words)
                .collect();
            if !needs.is_empty() {
                fields.insert("needs".into(), needs.into());
            }
        }
        if !self.asserts.is_empty() {
            let asserts: Vec<Value> = (self.asserts.iter())
                .map(|named| {
                    let mut listed = serde_json::json!({ "assertion": named.id, "evidence": named.evidence, "met": self.asserted(named) });
                    if self.unapproved(named) {
                        listed["awaitsApproval"] = true.into();
                    }
                    listed
                })
                .collect();
            fields.insert("asserts".into(), asserts.into());
        }
    }
}

pub(super) use crate::git::short;

/// How the project checks a slice's work.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Checking {
    Nothing,
    /// Its `sikemux.json` is invalid; says what is wrong.
    Unread(String),
    /// Its check command runs on commits on the project's branch.
    Command,
    /// Its CI runs on the project's branch as pushed; `pushed` says whether
    /// that holds the slice's head.
    Ci {
        pushed: bool,
    },
}

/// What git and the project's files say of a slice, read before the board is locked.
pub(super) struct Seen {
    pub checking: Checking,
    /// Whether git confirms the slice's head is its owner's latest commit, or why not.
    confirmed: Result<(), String>,
    /// The card's `updated_at` when it was read; what was read holds only
    /// while the card has not changed since.
    read: Option<u64>,
}

impl Seen {
    pub(super) fn unread() -> Self {
        Self {
            checking: Checking::Nothing,
            confirmed: Err("Sikemux did not read it from git".into()),
            read: None,
        }
    }

    /// Whether git confirms `card`'s slice head, if the card has not changed since.
    pub(super) fn confirmed(&self, card: &Card) -> Result<(), &str> {
        self.confirmed.as_ref().map_err(String::as_str)?;
        if self.read != Some(card.updated_at) {
            return Err("the card changed while Sikemux read git; mark it done again");
        }
        Ok(())
    }
}

/// A slice's change from `base` to `head`, to collect toward `gate`.
struct DiffWork {
    project: String,
    card: u64,
    gate: Step,
    base: String,
    head: String,
}

/// What is left to do once the board's lock is let go of: git work and
/// the checker, which never run under it.
#[derive(Default)]
pub(super) struct Later {
    diffs: Vec<DiffWork>,
    /// Commits the project's check is to run on again: project, commit and
    /// the agent whose it is.
    rechecks: Vec<(String, String, String)>,
    /// Projects whose workflows are to be read again, each for the card whose
    /// commands are to run again.
    looks: Vec<(String, u64)>,
}

impl Later {
    pub(super) fn recheck(project: &str, sha: String, agent: &str) -> Self {
        Self {
            rechecks: vec![(project.to_owned(), sha, agent.to_owned())],
            ..Self::default()
        }
    }

    pub(super) fn look_again(project: &str, card: u64) -> Self {
        Self {
            looks: vec![(project.to_owned(), card)],
            ..Self::default()
        }
    }

    pub(super) fn and(mut self, other: Later) -> Self {
        self.diffs.extend(other.diffs);
        self.rechecks.extend(other.rechecks);
        self.looks.extend(other.looks);
        self
    }

    pub(super) fn run(self, core: &Core) {
        for (project, sha, agent) in self.rechecks {
            let subject = Git::quick(Path::new(&project))
                .trimmed(&["log", "-1", "--format=%s", &sha])
                .unwrap_or_default();
            core.checks.check_again(&project, &sha, subject, &agent);
        }
        for diff in self.diffs {
            collect_diff(core, diff);
        }
        for (project, card) in self.looks {
            super::workflows::look_again(core, &project);
            super::commands::run_again(core, &project, card);
        }
    }
}

/// Where a slice got to once its ladder moved on as far as it goes now.
pub(super) struct Moved {
    pub passed: Vec<Step>,
    /// Its last gate passed, so the card is done.
    pub done: bool,
    pub later: Later,
}

/// Moves the slice on `card` up its ladder as far as it goes now, after the
/// gates `passed` already. Marking the card done is the caller's.
pub(super) fn progress(
    project: &str,
    card: &mut Card,
    mut passed: Vec<Step>,
    settings: &BoardSettings,
) -> Moved {
    let owner = card.owner_id().map(str::to_owned);
    let Some(slice) = card.slice.as_mut() else {
        return Moved {
            passed,
            done: false,
            later: Later::default(),
        };
    };
    passed.extend(slice.advance(owner.as_deref(), settings));
    slice.note_built_in();
    let mut later = Later::default();
    if let (Some(gate), Some(work)) = (
        passed
            .iter()
            .find(|gate| rule(gate).needs.contains(&Need::PassingCheck)),
        &slice.work,
    ) {
        later.diffs.push(DiffWork {
            project: project.to_owned(),
            card: card.id,
            gate: gate.clone(),
            base: work.base.clone(),
            head: work.head.clone(),
        });
    }
    Moved {
        done: slice.waiting().is_none(),
        passed,
        later,
    }
}

/// What the owner hears of the gates its slice passed.
pub(super) fn passed_notice(
    card: &Card,
    passed: &[Step],
    settings: &BoardSettings,
) -> Option<Notice> {
    let owner = card.owner_id()?;
    if passed.is_empty() {
        return None;
    }
    let names: Vec<&str> = passed.iter().map(gate_words).collect();
    let then = match card.slice.as_ref().and_then(Slice::waiting) {
        None => "It is done.".to_owned(),
        Some(Step::Environment(environment)) => {
            format!(
                "Next is {environment}: deploy there; {}.",
                settings.passes_words(environment)
            )
        }
        Some(gate) if passer(gate, settings) == Passer::Person => {
            format!("It waits for the person at {}.", gate_words(gate))
        }
        Some(gate) => format!("Next is {}.", gate_words(gate)),
    };
    Some(Notice {
        to: owner.to_owned(),
        text: format!(
            "Your card {} passed {}. {then}",
            card.label(),
            names.join(", ")
        ),
    })
}

/// Collects a slice's change as Diff evidence, unless its work moved on
/// meanwhile, and has the page it shows taken when it changes the project's pages.
fn collect_diff(core: &Core, diff: DiffWork) {
    let (proof, pages) = match diff_of(Path::new(&diff.project), &diff.base, &diff.head) {
        Ok(collected) => collected,
        Err(error) => {
            eprintln!("sikemux core: the slice's change was not collected: {error}");
            return;
        }
    };
    let owner = {
        let mut state = core.boards.state.locked();
        let Some(card) = state
            .boards
            .get_mut(&diff.project)
            .and_then(|board| board.card_mut(diff.card).ok())
        else {
            return;
        };
        let owner = card.owner_id().map(str::to_owned);
        let Some(slice) = card
            .slice
            .as_mut()
            .filter(|slice| slice.head() == Some(diff.head.as_str()))
        else {
            return;
        };
        slice.add_evidence(&diff.gate, epoch_ms(), owner.clone(), None, proof, &[]);
        core.boards.save(&mut state, &diff.project);
        owner
    };
    if let (true, Some(owner)) = (pages, owner) {
        super::page::take(core, diff.project, diff.card, owner, diff.head);
    }
}

/// The change from `base` to `head`, and whether it touches the project's pages.
fn diff_of(repo: &Path, base: &str, head: &str) -> Result<(Proof, bool), String> {
    let numstat = Git::quick(repo).trimmed(&["diff", "--numstat", base, head])?;
    let (mut files, mut added, mut removed, mut pages) = (Vec::new(), 0, 0, false);
    for line in numstat.lines() {
        let mut parts = line.splitn(3, '\t');
        let (Some(plus), Some(minus), Some(path)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        added += plus.parse::<u64>().unwrap_or(0);
        removed += minus.parse::<u64>().unwrap_or(0);
        pages |= super::page::is_page_file(path);
        if files.len() < DIFF_FILES {
            files.push(path.to_owned());
        }
    }
    let proof = Proof::Diff {
        base: base.to_owned(),
        head: head.to_owned(),
        files,
        added,
        removed,
    };
    Ok((proof, pages))
}

/// What each commit's Sikemux footer names, kept as read so a commit is
/// read once; past its bound it starts over.
#[derive(Default)]
pub(super) struct Footers(std::sync::Mutex<std::collections::HashMap<String, Vec<String>>>);

impl Footers {
    fn names(&self, core: &Core, dir: &Path, sha: &str, agent: &str) -> bool {
        if let Some(names) = self.0.locked().get(sha) {
            return names.iter().any(|named| named == agent);
        }
        let names: Vec<String> = footer::verified(core, dir, sha)
            .unwrap_or_default()
            .into_iter()
            .map(|named| named.chat)
            .collect();
        let found = names.iter().any(|named| named == agent);
        let mut kept = self.0.locked();
        if kept.len() >= MAX_FOOTERS {
            kept.clear();
        }
        kept.insert(sha.to_owned(), names);
        found
    }
}

/// What git says of the slice on `id`'s head, read without the board's lock:
/// for the card its owner works now, that no newer commit of the owner's is in
/// the card's range; for any other, that git still holds it. In a shared
/// checkout whose commits carry no Sikemux footer, git can't tell whose a
/// commit is, so the head the board heard of the owner's own git_commit
/// stands once git still holds it.
pub(super) fn seen(core: &Core, project: &str, id: u64, check: &Result<Checker, NoCheck>) -> Seen {
    let read = {
        let state = core.boards.state.locked();
        state.boards.get(project).and_then(|board| {
            let card = board.card(id).ok()?;
            let owner = card.owner_id()?.to_owned();
            let work = card.slice.as_ref()?.work.clone()?;
            let current = board.working_card(&owner).map(|card| card.id) == Some(id);
            Some((owner, work, current, card.updated_at))
        })
    };
    let checking = |head: Option<&str>| match check {
        Err(NoCheck::Unnamed) => Checking::Nothing,
        Err(NoCheck::Invalid(why)) => Checking::Unread(why.clone()),
        Ok(Checker::Named(ProjectCheck::Command(_)) | Checker::Found(_)) => Checking::Command,
        Ok(Checker::Named(ProjectCheck::Ci { .. })) => Checking::Ci {
            pushed: head.is_some_and(|head| crate::git::pushed(Path::new(project), head)),
        },
    };
    let Some((owner, work, current, at)) = read else {
        return Seen {
            checking: checking(None),
            ..Seen::unread()
        };
    };
    let tells_latest = core
        .workspaces
        .agent(&owner)
        .is_some_and(|agent| agent.worktree.is_some())
        || footer::attributed(Path::new(project));
    let confirmed = match current && tells_latest {
        true => owner_latest(core, project, &owner, &work.base).and_then(|latest| {
            (latest == work.head).then_some(()).ok_or_else(|| {
                format!(
                    "your latest commit {} hasn't been checked yet; mark the card done again once it has",
                    short(&latest)
                )
            })
        }),
        false => Git::quick(Path::new(project))
            .trimmed(&["cat-file", "-e", &format!("{}^{{commit}}", work.head)])
            .map(drop)
            .map_err(|_| format!("{} is not in git any more", short(&work.head))),
    };
    Seen {
        checking: checking(Some(&work.head)),
        confirmed,
        read: Some(at),
    }
}

/// The owner's latest commit as git has it now, for work that starts on
/// `base`. In a checkout of its own: that checkout's head. In the project's
/// own: the newest commit since `base` on the branch whose Sikemux footer
/// names it, leaving out the commits that bring generated files up to date,
/// within the newest `OWNER_COMMITS_READ`.
fn owner_latest(core: &Core, project: &str, agent: &str, base: &str) -> Result<String, String> {
    if let Some(worktree) = core
        .workspaces
        .agent(agent)
        .and_then(|agent| agent.worktree)
    {
        return Git::quick(&worktree.path).trimmed(&["rev-parse", "HEAD"]);
    }
    let dir = Path::new(project);
    Git::quick(dir)
        .trimmed(&["merge-base", "--is-ancestor", base, "HEAD"])
        .map_err(|_| {
            format!(
                "{}, where your work starts, is not on the project's branch any more",
                short(base)
            )
        })?;
    let read = (OWNER_COMMITS_READ + 1).to_string();
    let log = Git::quick(dir).trimmed(&[
        "log",
        "-n",
        &read,
        "--format=%H%x00%s",
        &format!("{base}..HEAD"),
    ])?;
    let generated = crate::server::edits::generate::GENERATED_SUBJECT;
    let latest = log
        .lines()
        .take(OWNER_COMMITS_READ)
        .filter_map(|line| line.split_once('\0'))
        .filter(|(_, subject)| !subject.starts_with(generated))
        .map(|(sha, _)| sha)
        .find(|sha| core.boards.footers.names(core, dir, sha, agent));
    match latest {
        Some(latest) => Ok(latest.to_owned()),
        None if log.lines().count() > OWNER_COMMITS_READ => Err(format!(
            "more than {OWNER_COMMITS_READ} commits came since {}, and Sikemux reads no further back for yours",
            short(base)
        )),
        None => Err(format!(
            "no commit since {} names you in its Sikemux footer",
            short(base)
        )),
    }
}

/// The last `commits` commits the footer names `agent` on, as work on a slice it takes on only
/// now, such as a chat tracked as a goal after it committed them.
pub(super) fn earlier_work(
    core: &Core,
    project: &str,
    agent: &str,
    commits: usize,
) -> Option<Head> {
    let worktree = core
        .workspaces
        .agent(agent)
        .and_then(|agent| agent.worktree);
    let dir = worktree
        .as_ref()
        .map_or(Path::new(project), |tree| &tree.path);
    let read = OWNER_COMMITS_READ.to_string();
    let log = Git::quick(dir)
        .trimmed(&["log", "-n", &read, "--format=%H%x00%P"])
        .ok()?;
    let mine: Vec<(&str, &str)> = (log.lines())
        .filter_map(|line| line.split_once('\0'))
        .filter(|(sha, _)| core.boards.footers.names(core, dir, sha, agent))
        .take(commits)
        .collect();
    let (head, _) = mine.first()?;
    let (_, parents) = mine.last()?;
    Some(Head {
        from: parents.split_whitespace().next()?.to_owned(),
        head: (*head).to_owned(),
        on_branch: worktree.is_none(),
    })
}

/// Auto takes the card: it becomes a slice, promising what its title says
/// when it promised nothing, on the ladder for how Auto finishes it.
pub(super) fn auto_slice(card: &mut Card, settings: &BoardSettings) {
    let ladder = ladder_of(settings, settings.landing == Landing::Core);
    let recordings = std::mem::take(&mut card.recordings);
    let slice = card
        .slice
        .get_or_insert_with(|| Slice::new(card.title.clone(), recordings, None, settings));
    if slice.passes.is_empty() {
        slice.ladder = ladder;
    }
    slice.begin(epoch_ms());
}

pub(super) fn took_work(card: &mut Card, head: &Head) {
    let owner = card.owner_id().map(str::to_owned);
    if let Some(slice) = card.slice.as_mut() {
        slice.worked(head, true, owner.as_deref(), None);
    }
}

/// Auto's agent shipped `head`, made on `from`, which counts as its owner
/// saying the slice is done.
pub(super) fn auto_shipped(card: &mut Card, from: &str, head: &str) {
    let owner = card.owner_id().map(str::to_owned);
    if let Some(slice) = card.slice.as_mut() {
        let head = Head {
            from: from.to_owned(),
            head: head.to_owned(),
            on_branch: false,
        };
        slice.worked(&head, true, owner.as_deref(), None);
        slice.done_said = true;
    }
}

/// A check on the head Auto pushed for `agent` ended: its slice keeps the run
/// as evidence, and moves on as far as it can. Auto's own agent is not told,
/// since a message would start it on another turn.
pub(super) fn auto_checked(core: &Arc<Core>, project: &str, id: u64, agent: &str, run: HeardRun) {
    let (notices, later) = {
        let mut state = core.boards.state.locked();
        let Some(board) = state.boards.get_mut(project) else {
            return;
        };
        let Ok(card) = board.card_mut(id) else {
            return;
        };
        let owner = card.owner_id().map(str::to_owned);
        let checked = card
            .slice
            .as_mut()
            .is_some_and(|slice| slice.checked(owner.as_deref(), &run));
        if !checked {
            return;
        }
        card.updated_at = epoch_ms();
        let moved = super::advance(
            core,
            &mut state,
            project,
            &By::Auto("Auto"),
            id,
            &Seen::unread(),
        );
        core.boards.save(&mut state, project);
        match moved {
            Ok(moved) => moved,
            Err(_) => return,
        }
    };
    let notices = notices
        .into_iter()
        .filter(|notice| notice.to != agent)
        .collect();
    send_notices(core, notices);
    let core = core.clone();
    tokio::task::spawn_blocking(move || later.run(&core));
}

/// Keeps a picture the agent made on its slice, toward the gate it waits
/// at. None when the card is not a slice, so the picture is kept as the
/// card's recording; false when the slice is done and keeps none.
pub(super) fn picture(card: &mut Card, agent: &str, recording: &InboxRecording) -> Option<bool> {
    Some(card.slice.as_mut()?.picture(Some(agent), recording))
}

/// Evidence came for the slice on `id`, which `collect` keeps; the slice
/// moves on as far as it can, and its owner hears of the gates it passed.
pub(super) fn moved_by_evidence(
    core: &Arc<Core>,
    project: &str,
    id: u64,
    collect: impl FnOnce(&mut Slice),
) {
    let (notices, later) = {
        let mut state = core.boards.state.locked();
        let Some(slice) = (state.boards.get_mut(project))
            .and_then(|board| board.card_mut(id).ok())
            .and_then(|card| card.slice.as_mut())
        else {
            return;
        };
        collect(slice);
        let moved = super::advance(
            core,
            &mut state,
            project,
            &By::Auto("Board"),
            id,
            &Seen::unread(),
        );
        core.boards.save(&mut state, project);
        match moved {
            Ok(moved) => moved,
            Err(_) => return,
        }
    };
    send_notices(core, notices);
    let core = core.clone();
    tokio::task::spawn_blocking(move || later.run(&core));
}

/// What `refused`'s agent is told to do about the failed check that stopped the slice `id`: mend
/// it on a card filed for the fix, or with a commit when none can be.
pub(super) fn fix_for(core: &Core, board: &mut Board, id: u64, refused: &Refused) -> String {
    (refused.failed.as_deref())
        .and_then(|sha| file_fix(core, board, id, sha))
        .unwrap_or_else(|| "Mend it with a commit, then mark it done again.".to_owned())
}

/// Gives every open slice on `board` the ladder its project gives it now. A
/// gate it passed stays passed while its ladder still has it.
pub(super) fn settle_ladders(board: &mut Board) {
    let settings = board.settings.clone();
    for card in &mut board.cards {
        if card.status == CardStatus::Done {
            continue;
        }
        let merges = card.auto.is_some() && settings.landing == Landing::Core;
        if let Some(slice) = &mut card.slice {
            slice.take_ladder(ladder_of(&settings, merges));
        }
    }
}

/// What ran toward an environment, and how it went.
pub(crate) struct DeployRan {
    pub environment: String,
    pub what: String,
    /// None when it ran in a way the core can't follow to its end.
    pub verdict: Option<Verdict>,
    pub output: String,
    /// The person allowed it, which passes the environment's step once it went.
    pub by_person: bool,
}

/// A deploy `agent` took ended: the card it works now keeps it as evidence
/// toward its environment's step, and moves on as far as that lets it. A card
/// already done keeps the evidence and its ladder.
pub(crate) fn deployed(core: &Arc<Core>, project: &str, agent: &str, ran: DeployRan) {
    let card = {
        let state = core.boards.state.locked();
        (state.boards.get(project)).and_then(|board| {
            let id = board.working_card(agent)?.id;
            let card = board.card(id).ok()?;
            let slice = card.slice.as_ref()?;
            Some((
                id,
                card.status != CardStatus::Done,
                slice.head().map(str::to_owned),
            ))
        })
    };
    let Some((id, open, head)) = card else {
        return;
    };
    let DeployRan {
        environment,
        what,
        verdict,
        output,
        by_person,
    } = ran;
    moved_by_evidence(core, project, id, |slice| {
        let proof = Proof::Run {
            command: what,
            sha: head,
            output,
        };
        slice.deploy_ran(&environment, Some(agent), verdict, proof, open);
    });
    if by_person && verdict != Some(Verdict::Failed) {
        let changes = Changes {
            pass: Some(Step::Environment(environment)),
            ..Changes::default()
        };
        if let Ok((_, notices)) = change_card(core, project, &By::Person, id, changes) {
            send_notices(core, notices);
        }
    }
}

/// Whether the check stands for what `agent` deploys from `head`, the commit
/// its checkout is at: on the card it works now, else on that commit as the
/// board heard. Why not, when not.
pub(crate) fn check_stands(
    core: &Core,
    project: &str,
    agent: &str,
    head: Option<&str>,
) -> Result<(), String> {
    let state = core.boards.state.locked();
    let slice = (state.boards.get(project)).and_then(|board| {
        board
            .card(working_slice(board, agent)?)
            .ok()?
            .slice
            .as_ref()
    });
    let not_yet = |sha: &str| format!("it has not passed on {}", short(sha));
    if let Some(slice) = slice {
        return match (slice.met(Need::PassingCheck), slice.head()) {
            (true, _) => Ok(()),
            (false, Some(sha)) => Err(not_yet(sha)),
            (false, None) => Err("its card has no commit for the check to pass on".to_owned()),
        };
    }
    let head = head.ok_or("Sikemux couldn't read the commit it would deploy")?;
    let passed = (state.runs.get(project))
        .and_then(|runs| runs.of(head))
        .is_some_and(|run| run.verdict == Some(Verdict::Passed));
    passed.then_some(()).ok_or_else(|| not_yet(head))
}

#[derive(Serialize, Deserialize)]
pub struct BoardPassParams {
    pub project: String,
    pub card: u64,
    pub gate: Gate,
    /// The project's environment `gate` stands for; a step that is one passes only by this name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
}

/// What the person writes on a slice, on a piece of its evidence when given.
#[derive(Serialize, Deserialize)]
pub struct BoardNoteParams {
    pub project: String,
    pub card: u64,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<u64>,
}

/// The person passes the gate a slice waits at, when it is theirs to pass.
/// Answers with the board.
pub(crate) async fn person_pass(
    core: &Arc<Core>,
    params: BoardPassParams,
) -> Result<ProjectBoard, String> {
    let core = core.clone();
    if let Some(environment) = params.environment.clone() {
        let project = project_at(&core, &params.project)?;
        let owner = {
            let state = core.boards.state.locked();
            (state.boards.get(&project))
                .and_then(|board| board.card(params.card).ok()?.owner_id().map(str::to_owned))
        };
        let Some(owner) = owner else {
            return pass_step(core, params).await;
        };
        if !crate::server::ship::allow_held(&core, &owner, &project, &environment).await {
            let view = pass_step(core.clone(), params).await?;
            crate::server::ship::permit_environment(&core, &owner, &project, &environment).await;
            return Ok(view);
        }
        return Ok(project_view(&core, &project));
    }
    pass_step(core, params).await
}

/// Passes the step the person named, and what moves on from it.
async fn pass_step(core: Arc<Core>, params: BoardPassParams) -> Result<ProjectBoard, String> {
    blocking(move || {
        let project = project_at(&core, &params.project)?;
        let changes = Changes {
            pass: Some(Step::named(params.gate, params.environment.as_deref())),
            ..Changes::default()
        };
        let (_, notices) = change_card(&core, &project, &By::Person, params.card, changes)?;
        send_notices(&core, notices);
        Ok(project_view(&core, &project))
    })
    .await
    .map_err(|error| error.to_string())
}

/// The person writes on a slice, on a piece of its evidence when given. The
/// note goes to the agent whose work that evidence shows, else to the slice's
/// owner, as a message from the person. Written at a gate of theirs, it says the
/// slice needs work, so the slice goes back to its owner until it is done again.
/// Answers with the board.
pub(crate) async fn person_note(
    core: &Arc<Core>,
    params: BoardNoteParams,
) -> Result<ProjectBoard, String> {
    let core = core.clone();
    blocking(move || {
        let project = project_at(&core, &params.project)?;
        let id = params.card;
        let text = params.text.trim().to_owned();
        if text.is_empty() || text.chars().count() > MAX_NOTE_CHARS {
            return Err(
                format!("text must be a note of at most {MAX_NOTE_CHARS} characters").into(),
            );
        }
        let (to, message) = {
            let mut state = core.boards.state.locked();
            let board = state
                .boards
                .get_mut(&project)
                .ok_or_else(|| format!("There is no card #{id} on this project's board"))?;
            let settings = board.settings.clone();
            let card = board.card_mut(id)?;
            let label = card.label();
            let owner = card.owner_id().map(str::to_owned);
            let slice = card
                .slice
                .as_mut()
                .ok_or_else(|| format!("{label} is not a slice, so it takes no notes"))?;
            let needs_work = slice.waits_on_person(&settings).is_some();
            let (agent, about) = slice
                .note(text.clone(), params.evidence)
                .map_err(|error| format!("{label}: {error}"))?;
            let mut message = format!("On card {label}, about {about}:\n\n{text}");
            if needs_work {
                slice.back_to_work();
                message.push_str("\n\nThe card is back with you: commit what the note asks, or answer it, then mark the card done again.");
            }
            card.updated_at = epoch_ms();
            core.boards.save(&mut state, &project);
            (agent.or(owner), message)
        };
        if let Some(to) = to {
            messages::from_person(&core, &to, message)?;
        }
        Ok(project_view(&core, &project))
    })
    .await
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::super::environments::{Environment, Passes, Recognise};
    use super::*;

    fn settings() -> BoardSettings {
        let environment = |name: &str, passes| Environment {
            name: name.to_owned(),
            passes,
            no_step: false,
            recognise: vec![Recognise::RundeckJob {
                path: format!("{name}/*"),
            }],
            health: None,
        };
        BoardSettings {
            environments: vec![
                environment("Dev", Passes::Nobody),
                environment("Staging", Passes::Check),
                environment("Production", Passes::Production),
            ],
            ..BoardSettings::default()
        }
    }

    fn head(sha: &str, on_branch: bool) -> Head {
        Head {
            from: "base".to_owned(),
            head: sha.to_owned(),
            on_branch,
        }
    }

    fn passed_built(settings: &BoardSettings) -> Slice {
        let mut slice = Slice::new("It works.".to_owned(), Vec::new(), None, settings);
        slice.worked(&head("abc", true), true, Some("owner"), None);
        slice.checked(
            Some("owner"),
            &HeardRun::of("abc", "check", Verdict::Passed, "ok"),
        );
        slice.done_said = true;
        assert_eq!(
            slice.advance(Some("owner"), settings),
            [Step::Gate(Gate::Built)]
        );
        slice
    }

    fn deploy(slice: &mut Slice, environment: &str, verdict: Option<Verdict>) {
        let proof = Proof::Run {
            command: format!("deploy {environment}"),
            sha: slice.head().map(str::to_owned),
            output: String::new(),
        };
        slice.deploy_ran(environment, Some("owner"), verdict, proof, true);
    }

    #[test]
    fn with_nothing_set_a_slice_takes_the_three_gates() {
        let settings = BoardSettings::default();
        let slice = Slice::new("It works.".to_owned(), Vec::new(), None, &settings);
        assert_eq!(
            serde_json::to_value(&slice.ladder).unwrap(),
            serde_json::json!(["built", "looksRight", "landed"])
        );
    }

    #[test]
    fn a_project_with_environments_takes_a_step_for_each_in_order() {
        let slice = Slice::new("It works.".to_owned(), Vec::new(), None, &settings());
        let names: Vec<&str> = slice.ladder.iter().map(gate_words).collect();
        assert_eq!(names, ["Built", "Dev", "Staging", "Production"]);
    }

    #[test]
    fn where_the_person_lands_slices_landed_stays_on_the_ladder() {
        let settings = BoardSettings {
            landing: Landing::Person,
            ..settings()
        };
        let slice = Slice::new("It works.".to_owned(), Vec::new(), None, &settings);
        let names: Vec<&str> = slice.ladder.iter().map(gate_words).collect();
        assert_eq!(names, ["Built", "Landed", "Dev", "Staging", "Production"]);
    }

    #[test]
    fn a_step_passes_only_on_a_deploy_there_and_production_on_the_person() {
        let settings = settings();
        let mut slice = passed_built(&settings);
        assert!(slice.advance(Some("owner"), &settings).is_empty());

        deploy(&mut slice, "Dev", Some(Verdict::Passed));
        assert_eq!(
            slice.advance(Some("owner"), &settings),
            [Step::Environment("Dev".to_owned())]
        );

        deploy(&mut slice, "Staging", Some(Verdict::Passed));
        slice.advance(Some("owner"), &settings);
        assert_eq!(
            slice.waiting(),
            Some(&Step::Environment("Production".to_owned()))
        );

        deploy(&mut slice, "Production", Some(Verdict::Passed));
        assert!(slice.advance(Some("owner"), &settings).is_empty());
        assert!(slice.waits_on_person(&settings).is_some());
    }

    #[test]
    fn a_failed_deploy_passes_nothing_and_marks_the_slice_failed() {
        let settings = settings();
        let mut slice = passed_built(&settings);
        deploy(&mut slice, "Dev", Some(Verdict::Failed));
        assert!(slice.advance(Some("owner"), &settings).is_empty());
        assert_eq!(slice.state(false, &settings), SliceState::Failed);
    }

    #[test]
    fn a_new_commit_undoes_what_was_deployed() {
        let settings = settings();
        let mut slice = passed_built(&settings);
        deploy(&mut slice, "Dev", Some(Verdict::Passed));
        slice.advance(Some("owner"), &settings);
        slice.worked(&head("def", true), true, Some("owner"), None);
        assert_eq!(slice.waiting(), Some(&Step::Gate(Gate::Built)));
        slice.checked(
            Some("owner"),
            &HeardRun::of("def", "check", Verdict::Passed, "ok"),
        );
        slice.done_said = true;
        assert_eq!(
            slice.advance(Some("owner"), &settings),
            [Step::Gate(Gate::Built)]
        );
    }

    #[test]
    fn a_step_that_needs_the_check_still_needs_it() {
        let settings = settings();
        let mut slice = passed_built(&settings);
        deploy(&mut slice, "Dev", Some(Verdict::Passed));
        slice.advance(Some("owner"), &settings);
        if let Some(work) = &mut slice.work {
            work.check = None;
        }
        deploy(&mut slice, "Staging", Some(Verdict::Passed));
        assert!(slice.advance(Some("owner"), &settings).is_empty());
        assert_eq!(slice.awaits_check(&settings), Some("abc"));
    }

    #[test]
    fn a_client_that_knows_no_environments_cannot_pass_one_by_its_gate() {
        let settings = settings();
        let mut slice = passed_built(&settings);
        deploy(&mut slice, "Dev", Some(Verdict::Passed));
        deploy(&mut slice, "Staging", Some(Verdict::Passed));
        slice.advance(Some("owner"), &settings);
        let said = slice
            .pass_by_person(&Step::Gate(Gate::Landed), "#1", &settings)
            .unwrap_err();
        assert!(said.contains("waits at Production"), "{said}");
    }

    #[test]
    fn a_released_client_reads_an_environment_as_landed_beside_its_name() {
        let settings = settings();
        let view = passed_built(&settings).view(None, false, |_| String::new(), None, &settings);
        let gates = serde_json::to_value(&view.gates).unwrap();
        assert_eq!(gates[1]["gate"], "landed");
        assert_eq!(gates[1]["environment"], "Dev");
        assert_eq!(gates[0]["gate"], "built");
        assert!(gates[0].get("environment").is_none());
    }

    #[test]
    fn taking_a_new_ladder_keeps_what_stays_and_drops_what_went() {
        let settings = settings();
        let mut slice = passed_built(&settings);
        deploy(&mut slice, "Dev", Some(Verdict::Passed));
        slice.advance(Some("owner"), &settings);
        slice.take_ladder(ladder_of(&BoardSettings::default(), false));
        assert_eq!(slice.waiting(), Some(&Step::Gate(Gate::LooksRight)));
    }
}
