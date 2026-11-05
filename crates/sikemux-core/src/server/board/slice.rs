//! A card that is a slice of a goal: the promise it makes, its ladder of
//! gates, and the evidence the core collected toward them. One table says who
//! passes each gate and what it needs first; views, passing and the inbox all
//! read it. An agent saying its slice is done passes nothing by itself; only
//! what the core collected can, checked against git as it is then. The board
//! follows the project's commits, landings and checks as repository events,
//! never by being called from git, and when it may have missed one it holds
//! back rather than pass.

use std::collections::{BTreeSet, VecDeque};
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_wire::inbox::InboxRecording;
use sikemux_wire::slice::{
    Asserted, Collect, Evidence, Gate, GateState, GateView, Need, NextGate, Note, Passer, Proof,
    SliceState, SliceView, Verdict, Wait,
};

use super::auto::{Landing, Settings};
use super::environments::{ladder_steps, Passes, Step};
use super::{
    change_card, change_in, messages, project_at, project_view, send_notices, Board, BoardView, By,
    Card, Changes, Checker, Notice, State, Status,
};
use crate::git::Git;
use crate::server::connection::blocking;
use crate::server::edits::footer;
use crate::server::repo_events::{RepoEvent, Told};
use crate::server::Core;
use crate::tasks::config::ProjectCheck;

/// The ladder a card takes once it makes a promise, in a project with no environments.
const LADDER: [Gate; 3] = [Gate::Built, Gate::LooksRight, Gate::Landed];
/// The ladder of a card Auto merges itself once its reviewer approves, which
/// leaves the person nothing to pass.
const AUTO_MERGES: [Gate; 2] = [Gate::Built, Gate::Landed];

/// The ladder a project gives its slices: one Auto merges itself leaves out
/// Looks right, and its environments, when it has them, take the place of
/// what follows Built.
pub(super) fn ladder_of(settings: &Settings, auto_merges: bool) -> Vec<Step> {
    if !settings.environments.is_empty() {
        return ladder_steps(settings.landing == Landing::Person, &settings.environments);
    }
    let gates: &[Gate] = if auto_merges { &AUTO_MERGES } else { &LADDER };
    gates.iter().copied().map(Step::Gate).collect()
}
/// Evidence kept besides the pieces notes are pinned to; the oldest goes first.
const MAX_EVIDENCE: usize = 24;
const MAX_NOTES: usize = 32;
/// What the board_add and board_update tools allow, which their check reads.
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

/// What a step must have before it can pass, and what it is called. An
/// environment's step is called and named by the environment's name.
pub(super) struct Rule<'a> {
    pub needs: &'static [Need],
    pub words: &'a str,
    /// What ids and agents name it by.
    pub key: &'a str,
}

pub(super) fn rule(step: &Step) -> Rule<'_> {
    match step {
        Step::Gate(Gate::Built) => Rule {
            needs: &[Need::PassingCheck, Need::Assertions],
            words: "Built",
            key: "built",
        },
        Step::Gate(Gate::LooksRight) => Rule {
            needs: &[],
            words: "Looks right",
            key: "looksRight",
        },
        Step::Gate(Gate::Landed) => Rule {
            needs: &[Need::OnBranch],
            words: "Landed",
            key: "landed",
        },
        Step::Environment(name) => Rule {
            needs: &[Need::OnBranch],
            words: name,
            key: name,
        },
    }
}

/// Who passes `step` in a project set as `settings`: the one table of who
/// passes what. An environment it no longer has is the person's.
pub(super) fn passer(step: &Step, settings: &Settings) -> Passer {
    match step {
        Step::Gate(Gate::Built) => Passer::Owner,
        Step::Gate(Gate::LooksRight) => Passer::Person,
        Step::Gate(Gate::Landed) => match settings.landing {
            Landing::Person => Passer::Person,
            Landing::Core => Passer::Core,
        },
        Step::Environment(name) => match settings.passes(name) {
            Some(Passes::Nobody | Passes::Check) => Passer::Core,
            Some(Passes::Person | Passes::Production) | None => Passer::Person,
        },
    }
}

pub(crate) fn gate_words(step: &Step) -> &str {
    rule(step).words
}

pub(crate) fn gate_key(step: &Step) -> &str {
    rule(step).key
}

/// What `step` needs in a project set as `settings`: its rule's, and the
/// check on top for an environment that goes once the check passes.
fn needs_of(step: &Step, settings: &Settings) -> Vec<Need> {
    let mut needs = rule(step).needs.to_vec();
    if let Step::Environment(name) = step {
        if settings.passes(name) == Some(Passes::Check) {
            needs.push(Need::PassingCheck);
        }
    }
    needs
}

/// The step a piece of evidence was collected toward.
fn toward(evidence: &Evidence) -> Step {
    Step::named(evidence.gate, evidence.environment.as_deref())
}

fn need_words(need: Need) -> &'static str {
    match need {
        Need::PassingCheck => "the project's check to pass on its latest commit",
        Need::OnBranch => "its commits on the project's branch",
        Need::Assertions => "evidence on its latest commit for each assertion it fulfils",
    }
}

pub(super) fn promise_of(text: &str) -> Result<String, String> {
    let promise = text.trim();
    if promise.is_empty() || promise.chars().count() > MAX_PROMISE_CHARS || promise.contains('\n') {
        return Err(format!(
            "promise must be one sentence of at most {MAX_PROMISE_CHARS} characters saying what the card changes"
        ));
    }
    Ok(promise.to_owned())
}

/// Who passed a gate.
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
    /// What the project's check said of `head`.
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
    done_said: bool,
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
}

/// An assertion of its goal a slice fulfils.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Named {
    /// The goal card whose assertion it is, so it binds no slice under another.
    pub goal: u64,
    pub id: u64,
    pub evidence: Collect,
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
    /// A new slice, keeping `recordings` the card had as pictures toward its
    /// first gate.
    pub(super) fn new(
        promise: String,
        recordings: Vec<InboxRecording>,
        owner: Option<&str>,
        settings: &Settings,
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
    pub(super) fn waits_on_person(&self, settings: &Settings) -> Option<&Step> {
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
    pub(super) fn awaits_check(&self, settings: &Settings) -> Option<&str> {
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
    pub(super) fn ahead(&self, gate: &Step, settings: &Settings) -> (Vec<&Step>, bool) {
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
    pub(super) fn failed_since(&self, since: u64, settings: &Settings) -> Option<bool> {
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
            return false;
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

    /// The ids of the assertions it fulfils.
    pub(super) fn asserts(&self) -> impl Iterator<Item = u64> + '_ {
        self.asserts.iter().map(|named| named.id)
    }

    /// The assertions it fulfils whose evidence is of `kind`.
    pub(super) fn asserting(&self, kind: Collect) -> Vec<u64> {
        (self.asserts.iter())
            .filter(|named| named.evidence == kind)
            .map(|named| named.id)
            .collect()
    }

    /// The latest evidence of its kind collected for the assertion.
    fn latest_for(&self, named: &Named) -> Option<&Evidence> {
        (self.evidence.iter().rev()).find(|evidence| {
            evidence.assertions.contains(&named.id)
                && match (&evidence.proof, &named.evidence) {
                    (Proof::Picture { seconds, .. }, Collect::Recording) => seconds.is_some(),
                    (Proof::Picture { seconds, .. }, Collect::Screenshot) => seconds.is_none(),
                    (Proof::Run { .. }, Collect::Workflow { .. }) => true,
                    _ => false,
                }
        })
    }

    /// The assertion holds on its latest commit: the check's verdict on it
    /// passed, for one the check shows; its workflow's run passed there, for
    /// one a workflow shows; else a recording or screenshot was collected for
    /// it there, which the person judges at Looks right.
    fn asserted(&self, named: &Named) -> bool {
        let on_head = self
            .latest_for(named)
            .filter(|evidence| evidence.commit.as_deref() == self.head());
        match named.evidence {
            Collect::Check => self.met(Need::PassingCheck),
            Collect::Workflow { .. } => {
                on_head.is_some_and(|evidence| evidence.verdict == Some(Verdict::Passed))
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
    /// stand there, with the evidence `standing` says each needs now; one that
    /// no longer stands is detached. Those of another goal, which it moved
    /// from, it is freed of. Whether it was.
    pub(super) fn bind(&mut self, goal: u64, standing: impl Fn(u64) -> Option<Collect>) -> bool {
        let moved = self.asserts.iter().any(|named| named.goal != goal);
        self.detached.retain(|named| named.goal == goal);
        for named in std::mem::take(&mut self.asserts) {
            match (named.goal == goal).then(|| standing(named.id)).flatten() {
                Some(evidence) => self.asserts.push(Named { evidence, ..named }),
                None if named.goal == goal && !self.detached.contains(&named) => {
                    self.detached.push(named)
                }
                None => {}
            }
        }
        moved
    }

    fn unmet(&self, gate: &Step, settings: &Settings) -> Vec<Need> {
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
            Some(work) if work.head == at.from && work.on_branch == at.on_branch => {
                work.base.clone()
            }
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
        if let Some(run) = runs.and_then(|runs| runs.of(&at.head)) {
            self.checked(owner, run);
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
    fn advance(&mut self, owner: Option<&str>, settings: &Settings) -> Vec<Step> {
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

    /// The latest deploy to the environment `step` is failed.
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
    pub(super) fn merged(&mut self, by_person: bool, settings: &Settings) -> bool {
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
    }

    /// The person passes `gate`, when it is the one waiting and theirs.
    pub(super) fn pass_by_person(
        &mut self,
        gate: &Step,
        label: &str,
        settings: &Settings,
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

    /// The owner says the slice is done. Refuses, changing nothing, when
    /// nothing the core collected can show its latest commit sound, or git
    /// does not confirm it is its latest: `confirmed`.
    pub(super) fn say_done(
        &mut self,
        label: &str,
        checking: Checking,
        confirmed: Result<(), &str>,
        settings: &Settings,
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
            return Err(format!(
                "Not {words}: you have committed nothing for {label}. Commit its work with git_commit, then mark it done once the project's check passes on it"
            )
            .into());
        };
        if let Err(why) = confirmed {
            return Err(format!("Not {words}: {why}").into());
        }
        let said = match (&work.check, self.unmet(&gate, settings).contains(&Need::PassingCheck)) {
            (_, false) => Said::Ready,
            (Some(Check { verdict: Some(Verdict::Failed), output, .. }), true) => {
                return Err(Refused {
                    text: format!(
                        "Not {words}: the project's check failed on {}, your latest commit.\n\n{output}",
                        short(&work.head)
                    ),
                    failed: Some(work.head.clone()),
                });
            }
            (_, true) if checking == Checking::Nothing => {
                return Err(format!(
                    "Not {words}: this project names no check in sikemux.json, neither a command nor CI, and Sikemux found no test command in its build files, so nothing can show the work sound. Ask your lead or the person how it is to be checked"
                )
                .into())
            }
            (_, true) if checking == (Checking::Ci { pushed: false }) => Said::Pending {
                note: format!(
                    "Not {words} yet: this project's check is its CI, which checks the project's branch as pushed, and {} isn't on it there yet. Land it if it isn't on the project's branch, then push the branch; {words} passes by itself once CI passes on it, and you will hear.",
                    short(&work.head)
                ),
                recheck: Some(work.head.clone()),
            },
            (_, true) if checking == (Checking::Ci { pushed: true }) => Said::Pending {
                note: format!(
                    "Not {words} yet: CI has not passed on {}, your latest commit. {words} passes by itself once it does, and you will hear.",
                    short(&work.head)
                ),
                recheck: Some(work.head.clone()),
            },
            (_, true) if work.on_branch => Said::Pending {
                note: format!(
                    "Not {words} yet: the project's check has not passed on {}, your latest commit. {words} passes by itself once it does, and you will hear.",
                    short(&work.head)
                ),
                recheck: Some(work.head.clone()),
            },
            (_, true) => Said::Pending {
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
        (self.met(Need::PassingCheck) && !missing.is_empty()).then(|| format!(
            "{label} is not Built yet: the check passed on {}, but {} no evidence there. Make what it asks for, a browser recording or a screenshot of the page, while this card is yours; Built passes by itself once each has it, and you will hear. board_list with the goal's card shows what each asks.",
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

    /// The files of the screenshots the core keeps as its evidence.
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
    fn state(&self, held_up: bool, settings: &Settings) -> SliceState {
        let Some(gate) = self.waiting() else {
            return SliceState::Done;
        };
        let failed = self
            .work
            .as_ref()
            .and_then(|work| work.check.as_ref())
            .is_some_and(|check| check.gate == *gate && check.verdict == Some(Verdict::Failed))
            || self.deploy_failed(gate);
        if failed {
            SliceState::Failed
        } else if self.stalled.is_some() || self.waits_on_person(settings).is_some() {
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
        settings: &Settings,
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
                })
                .collect(),
            detached: self.detached.iter().map(|named| named.id).collect(),
            wait,
        }
    }

    /// What `board_list` tells an agent of where it stands.
    pub(super) fn list_into(&self, fields: &mut Map<String, Value>, settings: &Settings) {
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
                    serde_json::json!({ "assertion": named.id, "evidence": named.evidence, "met": self.asserted(named) })
                })
                .collect();
            fields.insert("asserts".into(), asserts.into());
        }
    }
}

pub(super) fn short(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
}

/// Most pieces of evidence a pull request's description lists.
const PULL_EVIDENCE: usize = 12;

impl Slice {
    /// What a pull request opened for the slice's work says of its evidence, in
    /// Markdown: the newest pieces collected on its latest commit, the pictures and
    /// recordings by name and the host they are kept on, as no code host here takes
    /// uploads. None while it has none. What a check printed and where a file lies on
    /// the host stay out.
    pub(super) fn pull_evidence(&self, host: &str) -> Option<String> {
        let head = self.head();
        let on_head = |evidence: &&Evidence| head.is_none() || evidence.commit.as_deref() == head;
        let mut shown: Vec<&Evidence> = (self.evidence.iter().rev())
            .filter(on_head)
            .take(PULL_EVIDENCE)
            .collect();
        if shown.is_empty() {
            return None;
        }
        shown.reverse();
        let at = head.map_or_else(String::new, |head| format!(" for `{}`", short(head)));
        let mut text = format!(
            "### Evidence\n\nCollected by Sikemux on {host}{at}, where its pictures and recordings are kept with the card.\n\n"
        );
        for evidence in shown {
            text.push_str(&format!("- {}\n", pull_line(evidence)));
        }
        Some(text)
    }
}

/// One piece of evidence as a pull request's description lists it.
fn pull_line(evidence: &Evidence) -> String {
    let named = |path: &str| {
        Path::new(path).file_name().map_or_else(
            || path.to_owned(),
            |name| name.to_string_lossy().into_owned(),
        )
    };
    match &evidence.proof {
        Proof::Run { command, sha, .. } => {
            let went = match evidence.verdict {
                Some(Verdict::Passed) => "passed",
                Some(Verdict::Failed) => "failed",
                None => "gave no verdict",
            };
            let on = sha
                .as_deref()
                .map_or_else(String::new, |sha| format!(" on `{}`", short(sha)));
            format!("Check `{command}` {went}{on}")
        }
        Proof::Diff {
            base,
            head,
            files,
            added,
            removed,
        } => {
            let some: Vec<String> = files
                .iter()
                .take(3)
                .map(|file| format!("`{file}`"))
                .collect();
            let more = if files.len() > some.len() {
                " and more"
            } else {
                ""
            };
            format!(
                "Change `{}..{}`: +{added} −{removed} in {}{more}",
                short(base),
                short(head),
                some.join(", ")
            )
        }
        Proof::Picture {
            path,
            seconds: None,
            ..
        } => format!("Screenshot `{}`", named(path)),
        Proof::Picture {
            path,
            seconds: Some(seconds),
            ..
        } => format!("Recording `{}`, {seconds:.0} s", named(path)),
        Proof::Numbers { measures } => {
            let said: Vec<String> = (measures.iter())
                .map(|measure| match &measure.before {
                    Some(before) => format!("{} {before} → {}", measure.label, measure.after),
                    None => format!("{} {}", measure.label, measure.after),
                })
                .collect();
            format!("Numbers: {}", said.join("; "))
        }
        Proof::Timeline { moments } => format!("Timeline of {} events", moments.len()),
    }
}

fn proof_words(proof: &Proof) -> &'static str {
    match proof {
        Proof::Picture {
            seconds: Some(_), ..
        } => "recording",
        Proof::Picture { .. } => "screenshot",
        Proof::Diff { .. } => "change",
        Proof::Numbers { .. } => "numbers",
        Proof::Run { .. } => "check run",
        Proof::Timeline { .. } => "timeline",
    }
}

/// Whether every slice left in the goal lost its owner, so nothing in it moves until the person
/// or another agent takes one up.
pub(super) fn goal_stalled(board: &Board, goal: &Card) -> bool {
    let mut left = (board.held_by_goal(goal).into_iter())
        .filter(|card| card.status != Status::Done)
        .filter_map(|card| card.slice.as_ref())
        .peekable();
    left.peek().is_some() && left.all(|slice| slice.stalled.is_some())
}

/// How a goal's slices stand together, marked as one: failed when one
/// failed, needing the person when one does, moving when one moves, done when
/// every one is, else waiting. None for a card that is no goal.
pub(super) fn goal_state(board: &Board, goal: &Card) -> Option<SliceState> {
    let states: Vec<SliceState> = (board.held_by_goal(goal).into_iter())
        .filter_map(|card| {
            let held_up = board.held_up(card);
            Some(card.slice.as_ref()?.state(held_up, &board.settings))
        })
        .collect();
    let any = |state: SliceState| states.contains(&state);
    Some(match () {
        _ if states.is_empty() => return None,
        _ if any(SliceState::Failed) => SliceState::Failed,
        _ if any(SliceState::NeedsYou) => SliceState::NeedsYou,
        _ if any(SliceState::Moving) => SliceState::Moving,
        _ if states.iter().all(|state| *state == SliceState::Done) => SliceState::Done,
        _ => SliceState::Waiting,
    })
}

/// How a goal stands step by step for a chat's strip: one step per gate of a goal of one slice,
/// where each passed gate is done and the one it waits at stands as the slice does, else one per
/// slice, and whether the steps are gates.
pub(super) fn goal_steps(board: &Board, goal: &Card) -> (Vec<SliceState>, bool) {
    let cards: Vec<&Card> = (board.held_by_goal(goal).into_iter())
        .filter(|card| card.slice.is_some())
        .collect();
    let state_of = |card: &Card| {
        (card.slice.as_ref()).map(|slice| slice.state(board.held_up(card), &board.settings))
    };
    if let [card] = cards.as_slice() {
        let Some(slice) = card.slice.as_ref() else {
            return (Vec::new(), false);
        };
        let steps = (slice.ladder.iter())
            .map(|gate| {
                if slice.pass_of(gate).is_some() {
                    SliceState::Done
                } else if slice.waiting() == Some(gate) {
                    state_of(card).unwrap_or(SliceState::Waiting)
                } else {
                    SliceState::Waiting
                }
            })
            .collect();
        return (steps, true);
    }
    (cards.into_iter().filter_map(state_of).collect(), false)
}

/// The gates a goal's slices wait at, in the order they will be reached: a
/// slice that waits on another comes after it. Each says what it waits on now.
pub(super) fn next_gates(core: &Core, board: &Board, goal: &Card, now: u64) -> Vec<NextGate> {
    let under = board.held_by_goal(goal);
    let mut placed: Vec<u64> = Vec::new();
    let mut next = Vec::new();
    while placed.len() < under.len() {
        let ready: Vec<&Card> = under
            .iter()
            .copied()
            .filter(|card| !placed.contains(&card.id))
            .filter(|card| {
                card.depends_on.iter().all(|other| {
                    placed.contains(other) || !under.iter().any(|sibling| sibling.id == *other)
                })
            })
            .collect();
        if ready.is_empty() {
            break;
        }
        for card in ready {
            placed.push(card.id);
            if let Some(gate) = card.slice.as_ref().and_then(Slice::waiting) {
                next.push(NextGate {
                    card: card.id,
                    gate: gate.gate(),
                    environment: gate.environment().map(str::to_owned),
                    passer: passer(gate, &board.settings),
                    wait: super::wait::of(core, board, card, now),
                });
            }
        }
    }
    next
}

/// How the project checks a slice's work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Checking {
    /// It names no check.
    Nothing,
    /// Its check command runs on commits on the project's branch.
    Command,
    /// Its CI runs on the project's branch as pushed; `pushed` says whether
    /// that holds the slice's head.
    Ci { pushed: bool },
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
    /// Projects whose workflows are to be read again.
    looks: Vec<String>,
}

impl Later {
    pub(super) fn recheck(project: &str, sha: String, agent: &str) -> Self {
        Self {
            rechecks: vec![(project.to_owned(), sha, agent.to_owned())],
            ..Self::default()
        }
    }

    pub(super) fn look_again(project: &str) -> Self {
        Self {
            looks: vec![project.to_owned()],
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
            core.collisions.check_again(&project, &sha, subject, &agent);
        }
        for diff in self.diffs {
            collect_diff(core, diff);
        }
        for project in self.looks {
            super::workflows::look_again(core, &project);
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
    settings: &Settings,
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
pub(super) fn passed_notice(card: &Card, passed: &[Step], settings: &Settings) -> Option<Notice> {
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
pub(super) fn seen(core: &Core, project: &str, id: u64, check: Option<&Checker>) -> Seen {
    let read = {
        let state = core.boards.state.locked();
        state.boards.get(project).and_then(|board| {
            let card = board.card(id).ok()?;
            let owner = card.owner_id()?.to_owned();
            let work = card.slice.as_ref()?.work.clone()?;
            let current = current_card(board, &owner) == Some(id);
            Some((owner, work, current, card.updated_at))
        })
    };
    let checking = |head: Option<&str>| match check {
        None => Checking::Nothing,
        Some(Checker::Named(ProjectCheck::Command(_)) | Checker::Found(_)) => Checking::Command,
        Some(Checker::Named(ProjectCheck::Ci { .. })) => Checking::Ci {
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

/// The card `agent` works now on `board`: the one it last claimed or changed.
fn current_card(board: &Board, agent: &str) -> Option<u64> {
    board
        .cards
        .iter()
        .filter(|card| card.owner_id() == Some(agent))
        .max_by_key(|card| (card.status == Status::Progress, card.acted_at))
        .map(|card| card.id)
}

/// The card `agent` works now on `board`, when that is a slice in progress.
pub(super) fn working_slice(board: &Board, agent: &str) -> Option<u64> {
    let id = current_card(board, agent)?;
    let card = board.card(id).ok()?;
    let slice = match &card.slice {
        Some(_) => card,
        None => (board.cards.iter())
            .find(|slice| (slice.slice.as_ref()).is_some_and(|slice| slice.fix == Some(id)))?,
    };
    (slice.status == Status::Progress && slice.owner_id() == Some(agent)).then_some(slice.id)
}

/// The owner's work on the slice it works now moved on to `head`. The slice's id.
fn move_work(
    board: &mut Board,
    runs: Option<&HeardRuns>,
    agent: &str,
    head: &Head,
    new_work: bool,
    behind: impl Fn(&Card) -> bool,
) -> Option<u64> {
    let id = working_slice(board, agent)?;
    let card = board.card_mut(id).ok().filter(|card| !behind(card))?;
    card.slice
        .as_mut()?
        .worked(head, new_work, Some(agent), runs);
    card.updated_at = epoch_ms();
    Some(id)
}

/// Keeps every slice on `board` bound to the standing assertions of its goal
/// it names, with the evidence each needs now: the one place what a slice
/// must meet is settled, after its goal's assertions change, a card goes or
/// moves. The slices freed of assertions because they moved to another goal.
pub(super) fn rebind(board: &mut Board) -> Vec<u64> {
    let goals: Vec<(u64, u64)> = (board.cards.iter())
        .filter(|card| card.slice.is_some())
        .map(|card| (card.id, board.goal_above(card).id))
        .collect();
    let mut moved = Vec::new();
    for (id, goal) in goals {
        let standing: Vec<(u64, Collect)> = (board.card(goal).ok())
            .map(|goal| {
                (goal.goal.assertions().iter())
                    .filter(|assertion| assertion.retired.is_none())
                    .map(|assertion| (assertion.id, assertion.evidence.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let Some(slice) = board.card_mut(id).ok().and_then(|card| card.slice.as_mut()) else {
            continue;
        };
        let freed = slice.bind(goal, |id| {
            (standing.iter())
                .find(|(kept, _)| *kept == id)
                .map(|(_, evidence)| evidence.clone())
        });
        if freed {
            moved.push(id);
        }
    }
    moved
}

/// The project's check failed on `sha`, the latest commit of the slice on
/// `id`: the fix goes on a card under it, held by its owner, which the slice
/// waits on. A slice keeps one open fix card, which a later failure goes to.
/// What the owner is told of it.
pub(super) fn file_fix(core: &Core, board: &mut Board, id: u64, sha: &str) -> Option<String> {
    let slice = board.card(id).ok()?;
    let (owner, label) = (slice.owner_id()?.to_owned(), slice.label());
    let open = (slice.slice.as_ref()?.fix)
        .and_then(|fix| board.card(fix).ok())
        .filter(|fix| fix.status != Status::Done)
        .map(|fix| fix.id);
    let failed = format!(
        "The project's check failed on {}, the latest commit of {label}.",
        short(sha)
    );
    if let Some(fix) = open {
        let card = board.card_mut(fix).ok()?;
        card.details = format!("{failed} Mend it here; {label} waits on this card, and once it is done, mark {label} done again.");
        card.updated_at = epoch_ms();
        return Some(format!("Its fix goes on #{fix}, which is open already."));
    }
    if board.cards.len() >= super::MAX_CARDS {
        return None;
    }
    let settings = board.settings.clone();
    let fix = board.next.max(1);
    board.next = fix + 1;
    let mut card = super::new_card(
        core,
        fix,
        super::Adding {
            title: format!("Fix: the project's check failed on {}", short(sha)),
            details: format!("{failed} Mend it here; {label} waits on this card, and once it is done, mark {label} done again."),
            acceptance: String::new(),
            promise: None,
            asserts: Vec::new(),
            parent: Some(id),
            depends_on: Vec::new(),
            ticket: None,
            trigger: None,
            ci: None,
        },
        None,
        &settings,
    );
    super::take(core, &mut card, &owner);
    board.cards.push(card);
    let slice = board.card_mut(id).ok()?;
    let kept = slice.slice.as_mut()?;
    let done_fix = kept.fix.replace(fix);
    slice.depends_on.retain(|other| Some(*other) != done_fix);
    slice.depends_on.push(fix);
    Some(format!(
        "Sikemux put the fix on #{fix}, a card of yours under it, which {label} waits on: mend it there, mark #{fix} done, then mark {label} done again."
    ))
}

/// Follows the projects' commits, landings and checks for as long as the
/// core runs, each in the order it happened. What an earlier core left
/// unread is treated as lost.
pub(crate) async fn follow(core: Arc<Core>) {
    let every = core
        .boards
        .state
        .locked()
        .boards
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    core.repo_events.lost(every);
    let mut panics = 0;
    loop {
        let told = core.repo_events.next().await;
        let projects: BTreeSet<String> = told
            .events
            .iter()
            .map(|event| match event {
                RepoEvent::Committed { project, .. }
                | RepoEvent::Landed { project, .. }
                | RepoEvent::CheckStarted { project, .. }
                | RepoEvent::Checked { project, .. } => project.clone(),
            })
            .chain(told.lost.iter().cloned())
            .collect();
        let following = core.clone();
        let heard = tokio::task::spawn_blocking(move || heard_all(&following, told)).await;
        match heard {
            Ok(()) => panics = 0,
            Err(error) if panics < MAX_PANICS => {
                panics += 1;
                eprintln!("sikemux core: the board did not follow the repository, trying again: {error}");
                tokio::time::sleep(std::time::Duration::from_secs(1 << panics)).await;
                core.repo_events.lost(projects);
            }
            Err(error) => eprintln!(
                "sikemux core: the board gave up following {projects:?} after it failed {MAX_PANICS} times: {error}"
            ),
        }
    }
}

fn heard_all(core: &Arc<Core>, told: Told) {
    for project in &told.lost {
        lost(core, project);
    }
    for event in told.events {
        heard(core, event);
    }
}

/// Some of `project`'s events went unread, so no slice there passes on what
/// the board heard before: each holds what git says now, and is checked again.
fn lost(core: &Arc<Core>, project: &str) {
    let rechecks = {
        let state = core.boards.state.locked();
        let Some(board) = state.boards.get(project) else {
            return;
        };
        board
            .cards
            .iter()
            .filter(|card| card.status == Status::Progress)
            .filter_map(|card| {
                let work = card.slice.as_ref()?.work.as_ref()?;
                let owner = card.owner_id()?;
                let unchecked = work
                    .check
                    .as_ref()
                    .is_none_or(|check| check.verdict.is_none());
                (work.on_branch && unchecked)
                    .then(|| Later::recheck(project, work.head.clone(), owner))
            })
            .fold(Later::default(), Later::and)
    };
    rechecks.run(core);
}

/// Moves the slices `event` is about, and the cards they finish.
fn heard(core: &Arc<Core>, event: RepoEvent) {
    let from = match &event {
        RepoEvent::Committed { project, sha, .. } => Git::quick(Path::new(project))
            .trimmed(&["rev-parse", &format!("{sha}^")])
            .unwrap_or_else(|_| sha.clone()),
        RepoEvent::Landed { onto, .. } => onto.clone(),
        RepoEvent::CheckStarted { .. } | RepoEvent::Checked { .. } => String::new(),
    };
    let older = match &event {
        RepoEvent::Committed {
            project,
            agent,
            sha,
            ..
        }
        | RepoEvent::Landed {
            project,
            agent,
            tip: sha,
            ..
        } => {
            let known = {
                let state = core.boards.state.locked();
                state.boards.get(project).and_then(|board| {
                    let card = board.card(current_card(board, agent)?).ok()?;
                    card.slice.as_ref()?.head().map(str::to_owned)
                })
            };
            known.filter(|known| {
                known != sha
                    && Git::quick(Path::new(project))
                        .trimmed(&["merge-base", "--is-ancestor", sha, known])
                        .is_ok()
            })
        }
        RepoEvent::CheckStarted { .. } | RepoEvent::Checked { .. } => None,
    };
    let behind = |card: &Card| {
        older.is_some() && card.slice.as_ref().and_then(Slice::head) == older.as_deref()
    };
    let mut notices = Vec::new();
    let (project, touched) = {
        let mut state = core.boards.state.locked();
        let State { boards, runs, .. } = &mut *state;
        match event {
            RepoEvent::Committed {
                project,
                agent,
                sha,
                on_branch,
            } => {
                let head = Head {
                    from,
                    head: sha,
                    on_branch,
                };
                let runs = runs.get(&project);
                let Some(id) = boards
                    .get_mut(&project)
                    .and_then(|board| move_work(board, runs, &agent, &head, true, behind))
                else {
                    return;
                };
                (project, vec![id])
            }
            RepoEvent::Landed {
                project,
                agent,
                tip,
                ..
            } => {
                let head = Head {
                    from,
                    head: tip,
                    on_branch: true,
                };
                let runs = runs.get(&project);
                let Some(id) = boards
                    .get_mut(&project)
                    .and_then(|board| move_work(board, runs, &agent, &head, false, behind))
                else {
                    return;
                };
                (project, vec![id])
            }
            RepoEvent::CheckStarted {
                project,
                covered,
                run,
            } => {
                if let Some(board) = boards.get_mut(&project) {
                    board.checking.started(covered, run);
                }
                drop(state);
                core.boards.wake.now();
                return;
            }
            RepoEvent::Checked {
                project,
                command,
                sha,
                covered,
                verdict,
                output,
            } => {
                if let Some(board) = boards.get_mut(&project) {
                    board.checking.ended(&covered, verdict);
                }
                let run = HeardRun {
                    sha,
                    covered,
                    verdict,
                    command,
                    output,
                };
                let mut touched = Vec::new();
                if let Some(board) = boards.get_mut(&project) {
                    let checking: Vec<u64> = (board.cards.iter())
                        .filter(|card| card.status == Status::Progress && card.slice.is_some())
                        .map(|card| card.id)
                        .collect();
                    let mut refusals = Vec::new();
                    for id in checking {
                        let Ok(card) = board.card_mut(id) else {
                            continue;
                        };
                        let owner = card.owner_id().map(str::to_owned);
                        let label = card.label();
                        let Some(slice) = card.slice.as_mut() else {
                            continue;
                        };
                        if slice.checked(owner.as_deref(), &run) {
                            let waits = (slice.done_said)
                                .then(|| slice.awaits_evidence(&label))
                                .flatten();
                            match (slice.refused(&label), waits, owner) {
                                (Some(refused), _, Some(owner)) => {
                                    refusals.push((id, owner, refused))
                                }
                                (None, Some(text), Some(owner)) => {
                                    notices.push(Notice { to: owner, text })
                                }
                                _ => {}
                            }
                            card.updated_at = epoch_ms();
                            touched.push(id);
                        }
                    }
                    for (id, owner, refused) in refusals {
                        let fixed = (refused.failed.as_deref())
                            .and_then(|sha| file_fix(core, board, id, sha))
                            .unwrap_or_else(|| {
                                "Mend it with a commit, then mark it done again.".to_owned()
                            });
                        notices.push(Notice {
                            to: owner,
                            text: format!("{} {fixed}", refused.text),
                        });
                    }
                }
                runs.entry(project.clone()).or_default().keep(run);
                (project, touched)
            }
        }
    };
    if touched.is_empty() {
        send_notices(core, notices);
        return;
    }
    let seen: Vec<(u64, Seen)> = touched
        .into_iter()
        .map(|id| (id, seen(core, &project, id, None)))
        .collect();
    let mut later = Later::default();
    {
        let mut state = core.boards.state.locked();
        for (id, seen) in seen {
            match change_in(
                core,
                &mut state,
                &project,
                &By::Auto("Board"),
                id,
                Changes::default(),
                &seen,
            ) {
                Ok((_, sent, more)) => {
                    notices.extend(sent);
                    later = later.and(more);
                }
                Err(error) => eprintln!("sikemux core: the board did not follow #{id}: {error}"),
            }
        }
        core.boards.save(&mut state, &project);
    }
    later.run(core);
    send_notices(core, notices);
}

/// Auto takes the card: it becomes a slice, promising what its title says
/// when it promised nothing, on the ladder for how Auto finishes it.
pub(super) fn auto_slice(card: &mut Card, settings: &Settings) {
    let ladder = ladder_of(settings, settings.landing == Landing::Core);
    let recordings = std::mem::take(&mut card.recordings);
    let slice = card
        .slice
        .get_or_insert_with(|| Slice::new(card.title.clone(), recordings, None, settings));
    if slice.passes.is_empty() {
        slice.ladder = ladder;
    }
    slice.started = true;
    slice.started_at = Some(epoch_ms());
    slice.stalled = None;
}

/// The slice takes `head` as its work so far.
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
        let moved = change_in(
            core,
            &mut state,
            project,
            &By::Auto("Auto"),
            id,
            Changes::default(),
            &Seen::unread(),
        );
        core.boards.save(&mut state, project);
        match moved {
            Ok((_, notices, later)) => (notices, later),
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

/// The viewport presets, a phone's and a tablet's, a screenshot of a slice's page is also
/// taken at, so its evidence shows the page at those widths too.
pub(crate) const SCREENSHOT_WIDTHS: [&str; 2] = ["mobile", "tablet"];

/// Whether a screenshot `agent` takes now is kept as evidence: the card it works is a
/// slice with assertions a screenshot shows.
pub(crate) fn wants_screenshot(core: &Core, agent: &str) -> bool {
    let state = core.boards.state.locked();
    state.boards.values().any(|board| {
        working_slice(board, agent)
            .and_then(|id| board.card(id).ok()?.slice.as_ref())
            .is_some_and(|slice| !slice.asserting(Collect::Screenshot).is_empty())
    })
}

/// The agent took a screenshot, and the core took the same page beside it at each
/// width it is held at: when the card the agent works is a slice with assertions a
/// screenshot shows, the core keeps them all as their evidence, each in a file of its
/// own, and moves the slice on. `shots` are the browser's answers, the agent's first.
/// The board's writer removes a file once no evidence names it.
pub(crate) fn screenshot_taken(core: &Arc<Core>, agent: &str, shots: &[Value]) {
    let decoded = decode_shots(shots);
    if decoded.is_empty() {
        return;
    }
    let target = {
        let state = core.boards.state.locked();
        state.boards.iter().find_map(|(project, board)| {
            let id = working_slice(board, agent)?;
            let slice = board.card(id).ok()?.slice.as_ref()?;
            (!slice.asserting(Collect::Screenshot).is_empty()).then(|| (project.clone(), id))
        })
    };
    if let Some((project, id)) = target {
        keep_shots(core, &project, id, &decoded, |slice, kept| {
            slice.screenshots(agent, kept)
        });
    }
}

/// A screenshot kept as evidence: its file, and the width the page was laid out at.
pub(super) type Shot = (String, Option<u32>);

/// The pictures in the browser's answers to screenshots, each with the width
/// it was taken at.
pub(super) fn decode_shots(shots: &[Value]) -> Vec<(Vec<u8>, Option<u32>)> {
    use base64::Engine;
    shots
        .iter()
        .filter_map(|shot| {
            let bytes = (shot.get("data").and_then(Value::as_str))
                .and_then(|data| base64::engine::general_purpose::STANDARD.decode(data).ok())?;
            let width = shot
                .pointer("/viewport/width")
                .and_then(Value::as_u64)
                .and_then(|width| u32::try_from(width).ok());
            Some((bytes, width))
        })
        .collect()
}

/// Writes `decoded` each to a file of its own and has the slice on `id`
/// keep them with `collect`, then moves the slice on. The board's writer
/// removes a file once no evidence names it.
pub(super) fn keep_shots(
    core: &Arc<Core>,
    project: &str,
    id: u64,
    decoded: &[(Vec<u8>, Option<u32>)],
    collect: impl FnOnce(&mut Slice, &[Shot]),
) {
    let paths = {
        let mut state = core.boards.state.locked();
        let Some(paths) = (decoded.iter())
            .map(|(_, width)| core.boards.evidence_file(project, id, *width))
            .collect::<Option<Vec<_>>>()
        else {
            return;
        };
        if let Some(board) = state.boards.get_mut(project) {
            board.writing.extend(paths.iter().cloned());
        }
        paths
    };
    let mut kept = Vec::new();
    for ((bytes, width), path) in decoded.iter().zip(&paths) {
        let written = (path.parent())
            .map_or(Ok(()), std::fs::create_dir_all)
            .map_err(|error| error.to_string())
            .and_then(|()| {
                sikemux_process::files::write_atomic(path, bytes, true)
                    .map_err(|error| error.to_string())
            });
        match written {
            Ok(_) => kept.push((path.to_string_lossy().into_owned(), *width)),
            Err(error) => {
                eprintln!("sikemux core: a screenshot was not kept as evidence: {error}");
            }
        }
    }
    if !kept.is_empty() {
        moved_by_evidence(core, project, id, |slice| collect(slice, &kept));
    }
    if let Some(board) = core.boards.state.locked().boards.get_mut(project) {
        board.writing.retain(|writing| !paths.contains(writing));
    }
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
        let moved = change_in(
            core,
            &mut state,
            project,
            &By::Auto("Board"),
            id,
            Changes::default(),
            &Seen::unread(),
        );
        core.boards.save(&mut state, project);
        match moved {
            Ok((_, notices, later)) => (notices, later),
            Err(_) => return,
        }
    };
    send_notices(core, notices);
    let core = core.clone();
    tokio::task::spawn_blocking(move || later.run(&core));
}

/// Gives every open slice on `board` the ladder its project gives it now. A
/// gate it passed stays passed while its ladder still has it.
pub(super) fn settle_ladders(board: &mut Board) {
    let settings = board.settings.clone();
    for card in &mut board.cards {
        if card.status == Status::Done {
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
            let id = current_card(board, agent)?;
            let card = board.card(id).ok()?;
            let slice = card.slice.as_ref()?;
            Some((
                id,
                card.status != Status::Done,
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

/// The person passes a gate of a slice.
#[derive(Serialize, Deserialize)]
pub struct PassParams {
    pub project: String,
    pub card: u64,
    pub gate: Gate,
    /// The project's environment `gate` stands for; a step that is one passes only by this name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
}

/// What the person writes on a slice, on a piece of its evidence when given.
#[derive(Serialize, Deserialize)]
pub struct NoteParams {
    pub project: String,
    pub card: u64,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<u64>,
}

/// The person passes the gate a slice waits at, when it is theirs to pass.
/// Answers with the board.
pub(crate) async fn person_pass(core: &Arc<Core>, params: PassParams) -> Result<BoardView, String> {
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
async fn pass_step(core: Arc<Core>, params: PassParams) -> Result<BoardView, String> {
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
/// owner, as a message from the person; the gate keeps waiting. Answers with
/// the board.
pub(crate) async fn person_note(core: &Arc<Core>, params: NoteParams) -> Result<BoardView, String> {
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
            let card = state
                .boards
                .get_mut(&project)
                .ok_or_else(|| format!("There is no card #{id} on this project's board"))?
                .card_mut(id)?;
            let label = card.label();
            let owner = card.owner_id().map(str::to_owned);
            let (agent, about) = card
                .slice
                .as_mut()
                .ok_or_else(|| format!("{label} is not a slice, so it takes no notes"))?
                .note(text.clone(), params.evidence)
                .map_err(|error| format!("{label}: {error}"))?;
            card.updated_at = epoch_ms();
            core.boards.save(&mut state, &project);
            (
                agent.or(owner),
                format!("On card {label}, about {about}:\n\n{text}"),
            )
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

    fn settings() -> Settings {
        let environment = |name: &str, passes| Environment {
            name: name.to_owned(),
            passes,
            recognise: vec![Recognise::RundeckJob {
                path: format!("{name}/*"),
            }],
            health: None,
        };
        Settings {
            environments: vec![
                environment("Dev", Passes::Nobody),
                environment("Staging", Passes::Check),
                environment("Production", Passes::Production),
            ],
            ..Settings::default()
        }
    }

    fn head(sha: &str, on_branch: bool) -> Head {
        Head {
            from: "base".to_owned(),
            head: sha.to_owned(),
            on_branch,
        }
    }

    fn passed_built(settings: &Settings) -> Slice {
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
        let settings = Settings::default();
        let slice = Slice::new("It works.".to_owned(), Vec::new(), None, &settings);
        let expected = LADDER.map(Step::Gate);
        assert_eq!(slice.ladder, expected);
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
        let settings = Settings {
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
        slice.take_ladder(ladder_of(&Settings::default(), false));
        assert_eq!(slice.waiting(), Some(&Step::Gate(Gate::LooksRight)));
    }
}
