//! A card that is a slice of a goal: the promise it makes, its ladder of
//! gates, and the evidence the core collected toward them. One table says who
//! passes each gate and what it needs first; views, passing and the inbox all
//! read it. An agent saying its slice is done passes nothing by itself; only
//! what the core collected can. The board follows the project's commits,
//! landings and checks as repository events, never by being called from git.

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
    Card, Changes, Notice, Status,
};
use crate::git::Git;
use crate::server::connection::blocking;
use crate::server::repo_events::RepoEvent;
use crate::server::Core;

/// The ladder a card takes once it makes a promise.
const LADDER: [Gate; 3] = [Gate::Built, Gate::LooksRight, Gate::Landed];
/// Evidence kept besides the pieces notes are pinned to; the oldest goes first.
const MAX_EVIDENCE: usize = 24;
const MAX_NOTES: usize = 32;
const MAX_PROMISE_CHARS: usize = 200;
const MAX_NOTE_CHARS: usize = 4000;
const DIFF_FILES: usize = 8;

/// Who passes a gate, what must be so before it can, and what it is called.
pub(super) struct Rule {
    pub passer: Passer,
    pub needs: &'static [Need],
    pub words: &'static str,
    /// What ids name it by.
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum PassedBy {
    Agent(String),
    Person,
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
    /// as the commit or the landing that made `head` said.
    on_branch: bool,
    /// What the project's check said of `head`.
    check: Option<Check>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Check {
    /// The gate the slice waited at when it ran.
    gate: Gate,
    verdict: Verdict,
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

/// How the owner saying its slice is done went, when it was not refused.
pub(super) enum Said {
    Ready,
    /// Its gate passes by itself later; `recheck` is a commit to check again.
    Pending {
        note: String,
        recheck: Option<String>,
    },
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
                .is_some_and(|check| check.verdict == Verdict::Passed),
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
        agent: Option<String>,
        verdict: Option<Verdict>,
        proof: Proof,
    ) -> u64 {
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
            at: epoch_ms(),
            agent,
            verdict,
            proof,
        });
        id
    }

    fn picture(&mut self, agent: Option<&str>, recording: &InboxRecording) {
        let Some(gate) = self.waiting() else {
            return;
        };
        self.add_evidence(
            gate,
            agent.map(str::to_owned),
            None,
            Proof::Picture {
                path: recording.path.clone(),
                seconds: Some(recording.seconds),
                feed: recording.feed.clone(),
                turn: recording.turn,
            },
        );
    }

    /// The owner's work now ends at `head`, made on `from`. New work undoes
    /// every gate from the first that needs the check on it; work made again
    /// elsewhere, as landing does, keeps the gates it passed.
    fn worked(&mut self, from: &str, head: &str, on_branch: bool, new_work: bool) {
        let base = match &self.work {
            Some(work) if work.head == from && work.on_branch == on_branch => work.base.clone(),
            _ => from.to_owned(),
        };
        self.work = Some(Work {
            base,
            head: head.to_owned(),
            on_branch,
            check: None,
        });
        if !new_work {
            return;
        }
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

    /// The project's check ran, taking in `covered`. Whether it took in this
    /// slice's work, which then holds the run as evidence.
    fn checked(
        &mut self,
        owner: Option<&str>,
        covered: &[String],
        verdict: Verdict,
        run: Proof,
    ) -> bool {
        let (Some(gate), Some(head)) = (self.waiting(), self.work.as_ref().map(|work| &work.head))
        else {
            return false;
        };
        if !covered.contains(head) {
            return false;
        }
        self.add_evidence(gate, owner.map(str::to_owned), Some(verdict), run);
        if let Some(work) = &mut self.work {
            work.check = Some(Check { gate, verdict });
        }
        true
    }

    /// Passes each gate in turn whose passer may pass it now and whose needs
    /// are met. The gates passed.
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

    /// The person, or someone acting for them, closes the slice: every gate
    /// left passes as theirs.
    pub(super) fn close(&mut self) {
        while let Some(gate) = self.waiting() {
            self.passes.push(Pass {
                gate,
                by: PassedBy::Person,
                at: epoch_ms(),
            });
        }
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
    /// nothing the core collected can show the work sound.
    pub(super) fn say_done(&mut self, label: &str, has_check: bool) -> Result<Said, String> {
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
        let said = match (&self.work, self.unmet(gate).contains(&Need::PassingCheck)) {
            (_, false) => Said::Ready,
            (None, true) => {
                return Err(format!(
                    "Not {words}: you have committed nothing for {label}. Commit its work with git_commit, then mark it done once the project's check passes on it"
                ))
            }
            (
                Some(Work {
                    head,
                    check: Some(Check { verdict: Verdict::Failed, .. }),
                    ..
                }),
                true,
            ) => {
                let output = self
                    .evidence
                    .iter()
                    .rev()
                    .find_map(|evidence| match &evidence.proof {
                        Proof::Run { sha: Some(sha), output, .. } if sha == head => Some(output.as_str()),
                        _ => None,
                    })
                    .unwrap_or_default();
                return Err(format!(
                    "Not {words}: the project's check failed on {}, your latest commit. Mend it with a commit, then mark it done again.\n\n{output}",
                    short(head)
                ));
            }
            (Some(_), true) if !has_check => {
                return Err(format!(
                    "Not {words}: this project names no check command in sikemux.json, so nothing can show the work sound. Ask your lead or the person how it is to be checked"
                ))
            }
            (Some(Work { head, on_branch: true, .. }), true) => Said::Pending {
                note: format!(
                    "Not {words} yet: the project's check has not passed on {}, your latest commit. {words} passes by itself once it does, and you will hear.",
                    short(head)
                ),
                recheck: Some(head.clone()),
            },
            (Some(Work { head, .. }), true) => Said::Pending {
                note: format!(
                    "Not {words} yet: the project's check runs on the project's branch, and {} is only in your checkout. Land it with git_commit land true; {words} passes by itself once the check passes on it, and you will hear.",
                    short(head)
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
            .is_some_and(|check| check.gate == gate && check.verdict == Verdict::Failed);
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
            fields.insert("waitsAt".into(), gate_words(gate).into());
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

/// What is left to do once the board's lock is let go of: git work and
/// the checker, which never run under it.
#[derive(Default)]
pub(super) struct Later {
    /// A slice's work whose change becomes evidence toward the gate that just
    /// passed: project, card, gate, base and head.
    diff: Option<(String, u64, Gate, String, String)>,
    /// A slice's latest commit the project's check is to run on again: project,
    /// commit and the agent whose it is.
    recheck: Option<(String, String, String)>,
}

impl Later {
    pub(super) fn recheck(project: &str, sha: String, agent: &str) -> Self {
        Self {
            diff: None,
            recheck: Some((project.to_owned(), sha, agent.to_owned())),
        }
    }

    pub(super) fn and(self, other: Later) -> Self {
        Self {
            diff: self.diff.or(other.diff),
            recheck: self.recheck.or(other.recheck),
        }
    }

    pub(super) fn run(self, core: &Core) {
        if let Some((project, sha, agent)) = self.recheck {
            let subject = Git::quick(Path::new(&project))
                .trimmed(&["log", "-1", "--format=%s", &sha])
                .unwrap_or_default();
            core.collisions.check_again(&project, &sha, subject, &agent);
        }
        if let Some((project, id, gate, base, head)) = self.diff {
            collect_diff(core, &project, id, gate, &base, &head);
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
        later.diff = Some((
            project.to_owned(),
            card.id,
            gate,
            work.base.clone(),
            work.head.clone(),
        ));
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

/// Collects the change from `base` to `head` as Diff evidence toward `gate`,
/// unless the slice's work moved on meanwhile.
fn collect_diff(core: &Core, project: &str, id: u64, gate: Gate, base: &str, head: &str) {
    let proof = match diff_of(Path::new(project), base, head) {
        Ok(proof) => proof,
        Err(error) => {
            eprintln!("sikemux core: the slice's change was not collected: {error}");
            return;
        }
    };
    let mut state = core.boards.state.locked();
    let Some(card) = state
        .boards
        .get_mut(project)
        .and_then(|board| board.card_mut(id).ok())
    else {
        return;
    };
    let owner = card.owner_id().map(str::to_owned);
    let Some(slice) = card
        .slice
        .as_mut()
        .filter(|slice| slice.work.as_ref().is_some_and(|work| work.head == head))
    else {
        return;
    };
    slice.add_evidence(gate, owner, None, proof);
    core.boards.save(&mut state, project);
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

/// The slice `agent` works on `board`.
fn held_slice<'a>(board: &'a mut Board, agent: &str) -> Option<&'a mut Card> {
    board
        .cards
        .iter_mut()
        .filter(|card| {
            card.owner_id() == Some(agent)
                && card.status == Status::Progress
                && card.slice.is_some()
        })
        .max_by_key(|card| card.updated_at)
}

/// Follows the projects' commits, landings and checks for as long as the
/// core runs, each in the order it happened.
pub(crate) async fn follow(core: Arc<Core>) {
    loop {
        let events = core.repo_events.next().await;
        let core = core.clone();
        let heard = tokio::task::spawn_blocking(move || {
            for event in events {
                heard(&core, event);
            }
        })
        .await;
        if let Err(error) = heard {
            eprintln!("sikemux core: the board did not follow the repository: {error}");
        }
    }
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
    let mut state = core.boards.state.locked();
    let (project, touched) = match event {
        RepoEvent::Committed {
            project,
            agent,
            sha,
            on_branch,
        } => {
            let Some(card) = state
                .boards
                .get_mut(&project)
                .and_then(|board| held_slice(board, &agent))
            else {
                return;
            };
            if let Some(slice) = card.slice.as_mut() {
                slice.worked(&from, &sha, on_branch, true);
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
            let Some(card) = state
                .boards
                .get_mut(&project)
                .and_then(|board| held_slice(board, &agent))
            else {
                return;
            };
            if let Some(slice) = card.slice.as_mut() {
                slice.worked(&from, &tip, true, false);
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
            passed,
            output,
        } => {
            let Some(board) = state.boards.get_mut(&project) else {
                return;
            };
            let verdict = if passed {
                Verdict::Passed
            } else {
                Verdict::Failed
            };
            let mut touched = Vec::new();
            for card in board
                .cards
                .iter_mut()
                .filter(|card| card.status == Status::Progress)
            {
                let owner = card.owner_id().map(str::to_owned);
                let Some(slice) = card.slice.as_mut() else {
                    continue;
                };
                let run = Proof::Run {
                    command: command.clone(),
                    sha: sha.clone(),
                    output: output.clone(),
                };
                if slice.checked(owner.as_deref(), &covered, verdict, run) {
                    card.updated_at = epoch_ms();
                    touched.push(card.id);
                }
            }
            if touched.is_empty() {
                return;
            }
            (project, touched)
        }
    };
    let mut notices = Vec::new();
    let mut later = Later::default();
    for id in touched {
        match change_in(
            core,
            &mut state,
            &project,
            &By::Auto("Board"),
            id,
            Changes::default(),
            false,
        ) {
            Ok((_, sent, more)) => {
                notices.extend(sent);
                later = later.and(more);
            }
            Err(error) => eprintln!("sikemux core: the board did not follow #{id}: {error}"),
        }
    }
    core.boards.save(&mut state, &project);
    drop(state);
    later.run(core);
    send_notices(core, notices);
}

/// Keeps a picture the agent made on its slice, toward the gate it waits at.
/// False when the card is not a slice, so the picture is kept as the card's
/// recording.
pub(super) fn picture(card: &mut Card, agent: &str, recording: &InboxRecording) -> bool {
    let Some(slice) = card.slice.as_mut() else {
        return false;
    };
    slice.picture(Some(agent), recording);
    true
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
