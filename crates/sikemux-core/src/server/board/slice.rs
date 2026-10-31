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
    Evidence, Gate, GateState, GateView, Need, NextGate, Note, Passer, Proof, SliceState,
    SliceView, Verdict,
};

use super::{
    change_card, change_in, messages, project_at, project_view, send_notices, Board, BoardView, By,
    Card, Changes, Notice, State, Status,
};
use crate::git::Git;
use crate::server::connection::blocking;
use crate::server::edits::footer;
use crate::server::repo_events::{RepoEvent, Told};
use crate::server::Core;

/// The ladder a card takes once it makes a promise.
const LADDER: [Gate; 3] = [Gate::Built, Gate::LooksRight, Gate::Landed];
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

/// Who passes a gate, what must be so before it can, and what it is called.
pub(super) struct Rule {
    pub passer: Passer,
    pub needs: &'static [Need],
    pub words: &'static str,
    /// What ids and agents name it by.
    pub key: &'static str,
}

pub(super) fn rule(gate: Gate) -> Rule {
    match gate {
        Gate::Built => Rule {
            passer: Passer::Owner,
            needs: &[Need::PassingCheck],
            words: "Built",
            key: "built",
        },
        Gate::LooksRight => Rule {
            passer: Passer::Person,
            needs: &[],
            words: "Looks right",
            key: "looksRight",
        },
        Gate::Landed => Rule {
            passer: Passer::Core,
            needs: &[Need::OnBranch],
            words: "Landed",
            key: "landed",
        },
    }
}

pub(crate) fn gate_words(gate: Gate) -> &'static str {
    rule(gate).words
}

pub(crate) fn gate_key(gate: Gate) -> &'static str {
    rule(gate).key
}

fn need_words(need: Need) -> &'static str {
    match need {
        Need::PassingCheck => "the project's check to pass on its latest commit",
        Need::OnBranch => "its commits on the project's branch",
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
    gate: Gate,
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
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Check {
    /// The gate the slice waited at when it ran.
    gate: Gate,
    /// None when the check did not run.
    verdict: Option<Verdict>,
    /// The end of what it printed, or why it did not run.
    output: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Slice {
    pub promise: String,
    /// The gates it passes, in order.
    ladder: Vec<Gate>,
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
    ) -> Self {
        let mut slice = Self {
            promise,
            ladder: LADDER.to_vec(),
            passes: Vec::new(),
            evidence: Vec::new(),
            next_evidence: 1,
            notes: Vec::new(),
            work: None,
            done_said: false,
        };
        for recording in &recordings {
            slice.picture(owner, recording);
        }
        slice
    }

    fn pass_of(&self, gate: Gate) -> Option<&Pass> {
        self.passes.iter().find(|pass| pass.gate == gate)
    }

    pub(super) fn waiting(&self) -> Option<Gate> {
        self.ladder
            .iter()
            .copied()
            .find(|gate| self.pass_of(*gate).is_none())
    }

    /// The gate it waits at when that is the person's to pass and nothing it
    /// needs is missing.
    pub(super) fn waits_on_person(&self) -> Option<Gate> {
        self.waiting()
            .filter(|gate| rule(*gate).passer == Passer::Person && self.unmet(*gate).is_empty())
    }

    pub(super) fn head(&self) -> Option<&str> {
        self.work.as_ref().map(|work| work.head.as_str())
    }

    fn evidence(&self, id: u64) -> Option<&Evidence> {
        self.evidence.iter().find(|evidence| evidence.id == id)
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
        }
    }

    fn unmet(&self, gate: Gate) -> Vec<Need> {
        rule(gate)
            .needs
            .iter()
            .copied()
            .filter(|need| !self.met(*need))
            .collect()
    }

    /// Keeps a piece of evidence, letting the oldest go that no note is pinned to.
    fn add_evidence(
        &mut self,
        gate: Gate,
        at: u64,
        agent: Option<String>,
        verdict: Option<Verdict>,
        proof: Proof,
    ) {
        let notes = &self.notes;
        let pinned =
            |evidence: &Evidence| notes.iter().any(|note| note.evidence == Some(evidence.id));
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
            gate,
            at,
            agent,
            verdict,
            proof,
        });
    }

    /// Keeps a recording toward the gate it waits at. False when it waits at
    /// none, being done, so the recording is not kept.
    fn picture(&mut self, agent: Option<&str>, recording: &InboxRecording) -> bool {
        let Some(gate) = self.waiting() else {
            return false;
        };
        self.add_evidence(
            gate,
            recording.at,
            agent.map(str::to_owned),
            None,
            Proof::Picture {
                path: recording.path.clone(),
                seconds: Some(recording.seconds),
                feed: recording.feed.clone(),
                turn: recording.turn,
            },
        );
        true
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
        });
        if new_work {
            self.done_said = false;
            if let Some(from) = self
                .ladder
                .iter()
                .position(|gate| rule(*gate).needs.contains(&Need::PassingCheck))
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
        let (Some(gate), Some(head)) = (self.waiting(), self.head()) else {
            return false;
        };
        if !run.covered.iter().any(|covered| covered == head) {
            return false;
        }
        self.add_evidence(
            gate,
            epoch_ms(),
            owner.map(str::to_owned),
            run.verdict,
            Proof::Run {
                command: run.command.clone(),
                sha: run.sha.clone(),
                output: run.output.clone(),
            },
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
    /// which ends that it said so.
    fn refused(&mut self, label: &str) -> Option<String> {
        if !self.done_said {
            return None;
        }
        let gate = self.waiting()?;
        let work = self.work.as_ref()?;
        let check = work.check.as_ref()?;
        let text = match check.verdict {
            Some(Verdict::Passed) => return None,
            Some(Verdict::Failed) => format!(
                "Your card {label} did not pass {}: the project's check failed on {}. Mend it with a commit, then mark it done again.",
                gate_words(gate),
                short(&work.head)
            ),
            None => format!(
                "Your card {label} did not pass {}: the project's check did not run on {} ({}). Mark it done again to have it checked.",
                gate_words(gate),
                short(&work.head),
                check.output
            ),
        };
        self.done_said = false;
        Some(text)
    }

    /// Passes each gate in turn whose passer may pass it now and whose needs
    /// are met: the owner's once it said so, which git confirmed. The gates passed.
    fn advance(&mut self, owner: Option<&str>) -> Vec<Gate> {
        let mut passed = Vec::new();
        while let Some(gate) = self.waiting() {
            if !self.unmet(gate).is_empty() {
                break;
            }
            let by = match (rule(gate).passer, owner) {
                (Passer::Core, _) => PassedBy::Core,
                (Passer::Owner, Some(owner)) if self.done_said => PassedBy::Agent(owner.to_owned()),
                _ => break,
            };
            self.passes.push(Pass {
                gate,
                by,
                at: epoch_ms(),
            });
            passed.push(gate);
        }
        passed
    }

    /// Someone closes the slice: every gate left passes as `by`'s. The gates passed.
    pub(super) fn close(&mut self, by: PassedBy) -> Vec<Gate> {
        let mut passed = Vec::new();
        while let Some(gate) = self.waiting() {
            self.passes.push(Pass {
                gate,
                by: by.clone(),
                at: epoch_ms(),
            });
            passed.push(gate);
        }
        passed
    }

    /// It is back at work, so the gates it passed go.
    pub(super) fn restart(&mut self) {
        self.passes.clear();
        self.done_said = false;
    }

    /// Nobody holds it any more, so its work goes with the gates it passed.
    pub(super) fn reopen(&mut self) {
        self.restart();
        self.work = None;
    }

    /// The person passes `gate`, when it is the one waiting and theirs.
    pub(super) fn pass_by_person(&mut self, gate: Gate, label: &str) -> Result<(), String> {
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
        if self.waits_on_person() != Some(gate) {
            return Err(format!(
                "{} is not yours to pass on {label}",
                gate_words(gate)
            ));
        }
        self.passes.push(Pass {
            gate,
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
        has_check: bool,
        confirmed: Result<(), &str>,
    ) -> Result<Said, String> {
        let Some(gate) = self.waiting() else {
            return Err(format!("{label} is done already"));
        };
        if rule(gate).passer != Passer::Owner {
            return Err(format!(
                "{label} waits at {}, which is not yours to pass. If it needs more work, commit it and mark the card done again",
                gate_words(gate)
            ));
        }
        let words = gate_words(gate);
        let Some(work) = &self.work else {
            return Err(format!(
                "Not {words}: you have committed nothing for {label}. Commit its work with git_commit, then mark it done once the project's check passes on it"
            ));
        };
        if let Err(why) = confirmed {
            return Err(format!("Not {words}: {why}"));
        }
        let said = match (&work.check, self.unmet(gate).contains(&Need::PassingCheck)) {
            (_, false) => Said::Ready,
            (Some(Check { verdict: Some(Verdict::Failed), output, .. }), true) => {
                return Err(format!(
                    "Not {words}: the project's check failed on {}, your latest commit. Mend it with a commit, then mark it done again.\n\n{output}",
                    short(&work.head)
                ));
            }
            (_, true) if !has_check => {
                return Err(format!(
                    "Not {words}: this project names no check command in sikemux.json, so nothing can show the work sound. Ask your lead or the person how it is to be checked"
                ))
            }
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
        self.done_said = true;
        Ok(said)
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
                gate_words(on.gate)
            ),
            None => match self.waiting() {
                Some(gate) => format!("it, waiting at {}", gate_words(gate)),
                None => "it".to_owned(),
            },
        };
        Ok((on.and_then(|on| on.agent), about))
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
    fn state(&self, held_up: bool) -> SliceState {
        let Some(gate) = self.waiting() else {
            return SliceState::Done;
        };
        let failed = self
            .work
            .as_ref()
            .and_then(|work| work.check.as_ref())
            .is_some_and(|check| check.gate == gate && check.verdict == Some(Verdict::Failed));
        if failed {
            SliceState::Failed
        } else if self.waits_on_person().is_some() {
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
    ) -> SliceView {
        let waiting = self.waiting();
        let gates = self
            .ladder
            .iter()
            .map(|gate| {
                let rule = rule(*gate);
                let pass = self.pass_of(*gate);
                let agent = match (pass.map(|pass| &pass.by), rule.passer) {
                    (Some(PassedBy::Agent(agent)), _) => Some(agent.clone()),
                    (None, Passer::Owner) => owner.map(str::to_owned),
                    _ => None,
                };
                GateView {
                    gate: *gate,
                    passer: rule.passer,
                    state: match (pass, waiting == Some(*gate)) {
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
                        self.unmet(*gate)
                    },
                }
            })
            .collect();
        SliceView {
            promise: self.promise.clone(),
            state: self.state(held_up),
            gates,
            evidence: self.evidence.clone(),
            notes: self.notes.clone(),
        }
    }

    /// What `board_list` tells an agent of where it stands.
    pub(super) fn list_into(&self, fields: &mut Map<String, Value>) {
        fields.insert("promise".into(), self.promise.clone().into());
        if let Some(gate) = self.waiting() {
            fields.insert("waitsAt".into(), gate_key(gate).into());
            let needs: Vec<&str> = self.unmet(gate).into_iter().map(need_words).collect();
            if !needs.is_empty() {
                fields.insert("needs".into(), needs.into());
            }
        }
    }
}

fn short(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
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

/// The gates a goal's slices wait at, in the order they will be reached: a
/// slice that waits on another comes after it.
pub(super) fn next_gates(board: &Board, goal: &Card) -> Vec<NextGate> {
    let under: Vec<&Card> = board
        .cards
        .iter()
        .filter(|card| card.parent == Some(goal.id))
        .collect();
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
                    gate,
                    passer: rule(gate).passer,
                });
            }
        }
    }
    next
}

/// What git and the project's files say of a slice, read before the board is locked.
pub(super) struct Seen {
    /// The project names a check command.
    pub has_check: bool,
    /// Whether git confirms the slice's head is its owner's latest commit, or why not.
    confirmed: Result<(), String>,
    /// The card's `updated_at` when it was read; what was read holds only
    /// while the card has not changed since.
    read: Option<u64>,
}

impl Seen {
    pub(super) fn unread() -> Self {
        Self {
            has_check: false,
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
    gate: Gate,
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
}

impl Later {
    pub(super) fn recheck(project: &str, sha: String, agent: &str) -> Self {
        Self {
            diffs: Vec::new(),
            rechecks: vec![(project.to_owned(), sha, agent.to_owned())],
        }
    }

    pub(super) fn and(mut self, other: Later) -> Self {
        self.diffs.extend(other.diffs);
        self.rechecks.extend(other.rechecks);
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
    }
}

/// Where a slice got to once its ladder moved on as far as it goes now.
pub(super) struct Moved {
    pub passed: Vec<Gate>,
    /// Its last gate passed, so the card is done.
    pub done: bool,
    pub later: Later,
}

/// Moves the slice on `card` up its ladder as far as it goes now, after the
/// gates `passed` already. Marking the card done is the caller's.
pub(super) fn progress(project: &str, card: &mut Card, mut passed: Vec<Gate>) -> Moved {
    let owner = card.owner_id().map(str::to_owned);
    let Some(slice) = card.slice.as_mut() else {
        return Moved {
            passed,
            done: false,
            later: Later::default(),
        };
    };
    passed.extend(slice.advance(owner.as_deref()));
    let mut later = Later::default();
    if let (Some(gate), Some(work)) = (
        passed
            .iter()
            .copied()
            .find(|gate| rule(*gate).needs.contains(&Need::PassingCheck)),
        &slice.work,
    ) {
        later.diffs.push(DiffWork {
            project: project.to_owned(),
            card: card.id,
            gate,
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
pub(super) fn passed_notice(card: &Card, passed: &[Gate]) -> Option<Notice> {
    let owner = card.owner_id()?;
    if passed.is_empty() {
        return None;
    }
    let names: Vec<&str> = passed.iter().map(|gate| gate_words(*gate)).collect();
    let then = match card.slice.as_ref().and_then(Slice::waiting) {
        None => "It is done.".to_owned(),
        Some(gate) if rule(gate).passer == Passer::Person => {
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

/// Collects a slice's change as Diff evidence, unless its work moved on meanwhile.
fn collect_diff(core: &Core, diff: DiffWork) {
    let proof = match diff_of(Path::new(&diff.project), &diff.base, &diff.head) {
        Ok(proof) => proof,
        Err(error) => {
            eprintln!("sikemux core: the slice's change was not collected: {error}");
            return;
        }
    };
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
    slice.add_evidence(diff.gate, epoch_ms(), owner, None, proof);
    core.boards.save(&mut state, &diff.project);
}

fn diff_of(repo: &Path, base: &str, head: &str) -> Result<Proof, String> {
    let numstat = Git::quick(repo).trimmed(&["diff", "--numstat", base, head])?;
    let (mut files, mut added, mut removed) = (Vec::new(), 0, 0);
    for line in numstat.lines() {
        let mut parts = line.splitn(3, '\t');
        let (Some(plus), Some(minus), Some(path)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        added += plus.parse::<u64>().unwrap_or(0);
        removed += minus.parse::<u64>().unwrap_or(0);
        if files.len() < DIFF_FILES {
            files.push(path.to_owned());
        }
    }
    Ok(Proof::Diff {
        base: base.to_owned(),
        head: head.to_owned(),
        files,
        added,
        removed,
    })
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
/// the card's range; for any other, that git still holds it.
pub(super) fn seen(core: &Core, project: &str, id: u64, has_check: bool) -> Seen {
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
    let Some((owner, work, current, at)) = read else {
        return Seen {
            has_check,
            ..Seen::unread()
        };
    };
    let confirmed = match current {
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
        has_check,
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
fn working_slice<'a>(board: &'a mut Board, agent: &str) -> Option<&'a mut Card> {
    let id = current_card(board, agent)?;
    board
        .card_mut(id)
        .ok()
        .filter(|card| card.status == Status::Progress && card.slice.is_some())
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
        RepoEvent::Checked { .. } => String::new(),
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
        RepoEvent::Checked { .. } => None,
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
                let Some(card) = boards
                    .get_mut(&project)
                    .and_then(|board| working_slice(board, &agent))
                    .filter(|card| !behind(card))
                else {
                    return;
                };
                let head = Head {
                    from,
                    head: sha,
                    on_branch,
                };
                if let Some(slice) = card.slice.as_mut() {
                    slice.worked(&head, true, Some(&agent), runs.get(&project));
                }
                card.updated_at = epoch_ms();
                let id = card.id;
                (project, vec![id])
            }
            RepoEvent::Landed {
                project,
                agent,
                tip,
                ..
            } => {
                let Some(card) = boards
                    .get_mut(&project)
                    .and_then(|board| working_slice(board, &agent))
                    .filter(|card| !behind(card))
                else {
                    return;
                };
                let head = Head {
                    from,
                    head: tip,
                    on_branch: true,
                };
                if let Some(slice) = card.slice.as_mut() {
                    slice.worked(&head, false, Some(&agent), runs.get(&project));
                }
                card.updated_at = epoch_ms();
                let id = card.id;
                (project, vec![id])
            }
            RepoEvent::Checked {
                project,
                command,
                sha,
                covered,
                verdict,
                output,
            } => {
                let run = HeardRun {
                    sha,
                    covered,
                    verdict,
                    command,
                    output,
                };
                let mut touched = Vec::new();
                if let Some(board) = boards.get_mut(&project) {
                    for card in board
                        .cards
                        .iter_mut()
                        .filter(|card| card.status == Status::Progress)
                    {
                        let owner = card.owner_id().map(str::to_owned);
                        let label = card.label();
                        let Some(slice) = card.slice.as_mut() else {
                            continue;
                        };
                        if slice.checked(owner.as_deref(), &run) {
                            if let (Some(text), Some(owner)) = (slice.refused(&label), &owner) {
                                notices.push(Notice {
                                    to: owner.clone(),
                                    text,
                                });
                            }
                            card.updated_at = epoch_ms();
                            touched.push(card.id);
                        }
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
        .map(|id| (id, seen(core, &project, id, false)))
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

/// Keeps a picture the agent made on its slice, toward the gate it waits
/// at. None when the card is not a slice, so the picture is kept as the
/// card's recording; false when the slice is done and keeps none.
pub(super) fn picture(card: &mut Card, agent: &str, recording: &InboxRecording) -> Option<bool> {
    Some(card.slice.as_mut()?.picture(Some(agent), recording))
}

/// The person passes a gate of a slice.
#[derive(Serialize, Deserialize)]
pub struct PassParams {
    pub project: String,
    pub card: u64,
    pub gate: Gate,
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
    blocking(move || {
        let project = project_at(&core, &params.project)?;
        let changes = Changes {
            pass: Some(params.gate),
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
