//! A card that is a slice of a goal: the promise it makes, its ladder of
//! gates, and the evidence the core collected toward them. One table says who
//! passes each gate and what it needs first; views, passing and the inbox all
//! read it. An agent saying its slice is done passes nothing by itself; only
//! what the core collected can.

use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_wire::inbox::InboxRecording;
use sikemux_wire::slice::{
    Evidence, Gate, GateState, GateView, Need, NextGate, Note, Passer, Proof, SliceState,
    SliceView, Verdict,
};

use super::{
    held_card, may_change, messages, name_of, project_at, project_view, send_notices, unblocked,
    Board, BoardView, By, Card, Notice, Status,
};
use crate::git::Git;
use crate::server::connection::blocking;
use crate::server::Core;

/// The ladder a card takes once it makes a promise.
const LADDER: [Gate; 3] = [Gate::Built, Gate::LooksRight, Gate::Landed];
const MAX_EVIDENCE: usize = 24;
const MAX_NOTES: usize = 32;
const MAX_COMMITS: usize = 32;
const MAX_PROMISE_CHARS: usize = 200;
const MAX_NOTE_CHARS: usize = 4000;
const DIFF_FILES: usize = 8;

/// Who passes a gate, and what must be so before it can.
pub(super) struct Rule {
    pub passer: Passer,
    pub needs: &'static [Need],
}

pub(super) fn rule(gate: Gate) -> Rule {
    match gate {
        Gate::Built => Rule {
            passer: Passer::Owner,
            needs: &[Need::PassingCheck],
        },
        Gate::LooksRight => Rule {
            passer: Passer::Person,
            needs: &[],
        },
        Gate::Landed => Rule {
            passer: Passer::Core,
            needs: &[Need::OnBranch],
        },
    }
}

pub(crate) fn gate_words(gate: Gate) -> &'static str {
    match gate {
        Gate::Built => "Built",
        Gate::LooksRight => "Looks right",
        Gate::Landed => "Landed",
    }
}

fn need_words(need: Need) -> &'static str {
    match need {
        Need::PassingCheck => "the project's check to pass on its latest commit",
        Need::OnBranch => "its commits on the branch",
    }
}

/// The gate as ids and wire text name it.
pub(crate) fn gate_key(gate: Gate) -> String {
    serde_json::to_value(gate)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
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

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pass {
    gate: Gate,
    /// The agent that passed it; none for the person and the core.
    agent: Option<String>,
    at: u64,
}

/// A commit the slice's owner made for it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Commit {
    sha: String,
    /// On the branch, rather than only in the owner's own checkout.
    on_branch: bool,
    /// The latest run of the check that took it in.
    run: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Slice {
    pub promise: String,
    ladder: Vec<Gate>,
    #[serde(default)]
    passes: Vec<Pass>,
    #[serde(default)]
    evidence: Vec<Evidence>,
    #[serde(default)]
    next_evidence: u64,
    #[serde(default)]
    notes: Vec<Note>,
    /// Oldest first.
    #[serde(default)]
    commits: Vec<Commit>,
    /// Its owner said it is done since its latest commit, so the owner's gate
    /// passes as soon as what it needs is so.
    #[serde(default)]
    done_said: bool,
}

/// Why the check does not vouch for a slice's latest commit.
enum Unvouched {
    NoCommit,
    /// The check has not taken it in yet.
    Unchecked {
        sha: String,
        on_branch: bool,
    },
    Failed {
        sha: String,
        output: String,
    },
}

impl Slice {
    pub(super) fn new(promise: String) -> Self {
        Self {
            promise,
            ladder: LADDER.to_vec(),
            passes: Vec::new(),
            evidence: Vec::new(),
            next_evidence: 1,
            notes: Vec::new(),
            commits: Vec::new(),
            done_said: false,
        }
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
        match need {
            Need::PassingCheck => self
                .commits
                .last()
                .and_then(|head| head.run)
                .and_then(|run| self.evidence(run))
                .is_some_and(|run| run.verdict == Some(Verdict::Passed)),
            Need::OnBranch => {
                !self.commits.is_empty() && self.commits.iter().all(|commit| commit.on_branch)
            }
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

    fn add_evidence(
        &mut self,
        gate: Gate,
        agent: Option<String>,
        verdict: Option<Verdict>,
        proof: Proof,
    ) -> u64 {
        let id = self.next_evidence.max(1);
        self.next_evidence = id + 1;
        if self.evidence.len() >= MAX_EVIDENCE {
            self.evidence.remove(0);
        }
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

    /// The owner committed new work for it, which undoes every gate the old
    /// work passed from the first that needed the check on it.
    fn committed(&mut self, sha: &str, on_branch: bool) {
        if self.commits.len() >= MAX_COMMITS {
            self.commits.remove(0);
        }
        self.commits.push(Commit {
            sha: sha.to_owned(),
            on_branch,
            run: None,
        });
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

    /// Landing made its commits again on the branch, as `moved` pairs each
    /// old commit with the new one.
    fn landed(&mut self, moved: &[(String, String)]) -> bool {
        let mut changed = false;
        for commit in &mut self.commits {
            if let Some((_, new)) = moved.iter().find(|(old, _)| *old == commit.sha) {
                commit.sha.clone_from(new);
                commit.on_branch = true;
                changed = true;
            }
        }
        changed
    }

    /// Passes each gate in turn whose passer may pass it now and whose needs
    /// are met. The gates passed.
    fn advance(&mut self, owner: Option<&str>) -> Vec<Gate> {
        let mut passed = Vec::new();
        while let Some(gate) = self.waiting() {
            if !self.unmet(gate).is_empty() {
                break;
            }
            let agent = match rule(gate).passer {
                Passer::Core => None,
                Passer::Owner if self.done_said => owner.map(str::to_owned),
                Passer::Owner | Passer::Person => break,
            };
            self.passes.push(Pass {
                gate,
                agent,
                at: epoch_ms(),
            });
            passed.push(gate);
        }
        passed
    }

    fn unvouched(&self) -> Option<Unvouched> {
        if self.met(Need::PassingCheck) {
            return None;
        }
        let Some(head) = self.commits.last() else {
            return Some(Unvouched::NoCommit);
        };
        Some(match head.run.and_then(|run| self.evidence(run)) {
            Some(Evidence {
                proof: Proof::Run { output, .. },
                ..
            }) => Unvouched::Failed {
                sha: head.sha.clone(),
                output: output.clone(),
            },
            _ => Unvouched::Unchecked {
                sha: head.sha.clone(),
                on_branch: head.on_branch,
            },
        })
    }

    fn done(&self) -> bool {
        self.waiting().is_none()
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
            .evidence
            .iter()
            .rev()
            .find(|evidence| evidence.gate == gate && evidence.verdict.is_some())
            .is_some_and(|evidence| evidence.verdict == Some(Verdict::Failed));
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
                let agent = match (pass, rule.passer) {
                    (Some(pass), _) => pass.agent.clone(),
                    (None, Passer::Owner) => owner.map(str::to_owned),
                    (None, _) => None,
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
    pub(super) fn listed(&self) -> serde_json::Value {
        let mut view = serde_json::json!({ "promise": self.promise });
        if let (Some(gate), Some(fields)) = (self.waiting(), view.as_object_mut()) {
            fields.insert("waitsAt".into(), gate_words(gate).into());
            let needs: Vec<&str> = self.unmet(gate).into_iter().map(need_words).collect();
            if !needs.is_empty() {
                fields.insert("needs".into(), needs.into());
            }
        }
        view
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

/// Moves the slice on `id` up its ladder as far as it goes now, after the
/// gates `passed` already, marks the card done once its last gate passed, and
/// says who should hear of it.
fn advance(board: &mut Board, id: u64, mut passed: Vec<Gate>) -> (Vec<Gate>, Vec<Notice>) {
    let blocked_before = board.blocked();
    let Ok(card) = board.card_mut(id) else {
        return (Vec::new(), Vec::new());
    };
    let owner = card.owner_id().map(str::to_owned);
    let Some(slice) = card.slice.as_mut() else {
        return (Vec::new(), Vec::new());
    };
    passed.extend(slice.advance(owner.as_deref()));
    if passed.is_empty() {
        return (passed, Vec::new());
    }
    let done = slice.done();
    let next = slice.waiting();
    card.updated_at = epoch_ms();
    if done {
        card.status = Status::Done;
    }
    let label = card.label();
    let mut notices = Vec::new();
    if let Some(owner) = owner {
        let names: Vec<&str> = passed.iter().map(|gate| gate_words(*gate)).collect();
        let then = match next {
            None => "It is done.".to_owned(),
            Some(gate) if rule(gate).passer == Passer::Person => {
                format!("It waits for the person at {}.", gate_words(gate))
            }
            Some(gate) => format!("Next is {}.", gate_words(gate)),
        };
        notices.push(Notice {
            to: owner,
            text: format!("Your card {label} passed {}. {then}", names.join(", ")),
        });
    }
    if done {
        notices.extend(unblocked(board, &blocked_before, &By::Auto("Board")));
    }
    (passed, notices)
}

/// Collects the change the slice's commits make as Diff evidence toward Built,
/// once Built passed.
fn collect_diff(core: &Core, project: &str, id: u64) {
    let shas: Vec<String> = {
        let state = core.boards.state.locked();
        let Some(slice) = state
            .boards
            .get(project)
            .and_then(|board| board.card(id).ok())
            .and_then(|card| card.slice.as_ref())
        else {
            return;
        };
        slice
            .commits
            .iter()
            .map(|commit| commit.sha.clone())
            .collect()
    };
    let proof = match diff_of(Path::new(project), &shas) {
        Ok(Some(proof)) => proof,
        Ok(None) => return,
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
    if let Some(slice) = card.slice.as_mut() {
        slice.add_evidence(Gate::Built, owner, None, proof);
    }
    core.boards.save(&mut state, project);
}

/// The change `shas` make together, counted commit by commit so work others
/// landed between them is left out.
fn diff_of(repo: &Path, shas: &[String]) -> Result<Option<Proof>, String> {
    let (Some(first), Some(head)) = (shas.first(), shas.last()) else {
        return Ok(None);
    };
    let base = Git::quick(repo)
        .trimmed(&["rev-parse", &format!("{first}^")])
        .unwrap_or_else(|_| first.clone());
    let mut args = vec!["show", "--numstat", "--format="];
    args.extend(shas.iter().map(String::as_str));
    let numstat = Git::quick(repo).trimmed(&args)?;
    let (mut files, mut added, mut removed) = (Vec::new(), 0, 0);
    for line in numstat.lines() {
        let mut parts = line.splitn(3, '\t');
        let (Some(plus), Some(minus), Some(path)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        added += plus.parse::<u64>().unwrap_or(0);
        removed += minus.parse::<u64>().unwrap_or(0);
        if files.len() < DIFF_FILES && !files.iter().any(|file: &String| file == path) {
            files.push(path.to_owned());
        }
    }
    Ok(Some(Proof::Diff {
        base,
        head: head.clone(),
        files,
        added,
        removed,
    }))
}

/// Ends a change to a slice: saves its board, has Built's change collected
/// when Built just passed, and tells whoever should hear.
fn finish(core: &Arc<Core>, project: &str, id: u64, passed: &[Gate], notices: Vec<Notice>) {
    if passed.contains(&Gate::Built) {
        let (core, project) = (core.clone(), project.to_owned());
        tokio::task::spawn_blocking(move || collect_diff(&core, &project, id));
    }
    send_notices(core, notices);
}

/// `agent` committed `sha` for the slice it works, on the branch or only in
/// its own checkout.
pub(crate) fn committed(core: &Core, agent: &str, sha: &str, on_branch: bool) {
    let mut state = core.boards.state.locked();
    let Some((project, card)) = held_card(&mut state, agent) else {
        return;
    };
    let Some(slice) = card
        .slice
        .as_mut()
        .filter(|_| card.status == Status::Progress)
    else {
        return;
    };
    slice.committed(sha, on_branch);
    card.updated_at = epoch_ms();
    let project = project.clone();
    core.boards.save(&mut state, &project);
}

/// Landing put `agent`'s commits on the branch again as new ones, pairing
/// each old commit with its new one.
pub(crate) fn landed(core: &Arc<Core>, agent: &str, moved: &[(String, String)]) {
    let (project, id, passed, notices) = {
        let mut state = core.boards.state.locked();
        let Some((project, card)) = held_card(&mut state, agent) else {
            return;
        };
        let Some(slice) = card
            .slice
            .as_mut()
            .filter(|_| card.status == Status::Progress)
        else {
            return;
        };
        if !slice.landed(moved) {
            return;
        }
        let (project, id) = (project.clone(), card.id);
        let Some(board) = state.boards.get_mut(&project) else {
            return;
        };
        let (passed, notices) = advance(board, id, Vec::new());
        core.boards.save(&mut state, &project);
        (project, id, passed, notices)
    };
    finish(core, &project, id, &passed, notices);
}

/// The project's check ended, run at `sha`, taking in `covered` commits made
/// by `agents`: each agent's slice gets the run as evidence toward the gate
/// it waits at.
pub(crate) fn checked(
    core: &Arc<Core>,
    command: &str,
    sha: &str,
    covered: &[String],
    agents: &[String],
    passed: bool,
    output: &str,
) {
    let verdict = if passed {
        Verdict::Passed
    } else {
        Verdict::Failed
    };
    let mut moved = Vec::new();
    {
        let mut state = core.boards.state.locked();
        for agent in agents {
            let Some((project, card)) = held_card(&mut state, agent) else {
                continue;
            };
            let Some(slice) = card
                .slice
                .as_mut()
                .filter(|_| card.status == Status::Progress)
            else {
                continue;
            };
            let Some(gate) = slice.waiting() else {
                continue;
            };
            let run = slice.add_evidence(
                gate,
                Some(agent.clone()),
                Some(verdict),
                Proof::Run {
                    command: command.to_owned(),
                    sha: Some(sha.to_owned()),
                    output: output.to_owned(),
                },
            );
            for commit in &mut slice.commits {
                if covered.contains(&commit.sha) {
                    commit.run = Some(run);
                }
            }
            card.updated_at = epoch_ms();
            let (project, id) = (project.clone(), card.id);
            let Some(board) = state.boards.get_mut(&project) else {
                continue;
            };
            let (passed, notices) = advance(board, id, Vec::new());
            core.boards.save(&mut state, &project);
            moved.push((project, id, passed, notices));
        }
    }
    for (project, id, passed, notices) in moved {
        finish(core, &project, id, &passed, notices);
    }
}

/// Keeps a picture the agent made on its slice, toward the gate it waits at.
/// False when the agent's card is not a slice, so the picture is kept as the
/// card's recording.
pub(super) fn picture(card: &mut Card, agent: &str, recording: &InboxRecording) -> bool {
    let Some(slice) = card.slice.as_mut() else {
        return false;
    };
    if let Some(gate) = slice.waiting() {
        slice.add_evidence(
            gate,
            Some(agent.to_owned()),
            None,
            Proof::Picture {
                path: recording.path.clone(),
                seconds: Some(recording.seconds),
                feed: recording.feed.clone(),
                turn: recording.turn,
            },
        );
    }
    true
}

/// The owner says the slice on `id` is done. Passes the owner's gate when what
/// it needs is so; else keeps that it was said, so the gate passes once the
/// check passes, or refuses when nothing can show the work sound. What the
/// owner is told.
pub(super) fn said_done(
    core: &Arc<Core>,
    project: &str,
    agent: &str,
    id: u64,
) -> Result<String, String> {
    let has_check = super::check_command(core, project).is_some();
    let (passed, notices, note) = {
        let mut state = core.boards.state.locked();
        let board = state
            .boards
            .get_mut(project)
            .ok_or_else(|| format!("There is no card #{id} on this project's board"))?;
        let card = board.card_mut(id)?;
        if !may_change(core, agent, card.owner_id()) {
            let owner = name_of(core, card.owner_id().unwrap_or_default());
            return Err(format!(
                "#{id} is {owner}'s; message {owner} with agent_send to change it"
            ));
        }
        if card.status != Status::Progress {
            return Err(format!("Claim #{id} with board_claim to start on it"));
        }
        let label = card.label();
        let slice = card.slice.as_mut().ok_or("not a slice")?;
        let Some(gate) = slice.waiting() else {
            return Err(format!("{label} is done already"));
        };
        if rule(gate).passer != Passer::Owner {
            return Err(format!(
                "{label} waits at {}, which is not yours to pass. If it needs more work, commit it and mark the card done again",
                gate_words(gate)
            ));
        }
        let note = match slice.unvouched() {
            None => "Built passed. It stays in progress until the person says it looks right and it lands.".to_owned(),
            Some(Unvouched::NoCommit) => return Err(format!(
                "Not built: you have committed nothing for {label}. Commit its work with git_commit, then mark it done once the project's check passes on it"
            )),
            Some(Unvouched::Failed { sha, output }) => return Err(format!(
                "Not built: the project's check failed on {}, your latest commit. Mend it with a commit, then mark it done again.\n\n{output}",
                short(&sha)
            )),
            Some(Unvouched::Unchecked { .. }) if !has_check => return Err(
                "Not built: this project names no check command in sikemux.json, so nothing can show the work sound. Ask your lead or the person how it is to be checked".to_owned(),
            ),
            Some(Unvouched::Unchecked { sha, on_branch: true }) => format!(
                "Not built yet: the project's check has not finished on {}, your latest commit. Built passes by itself once it passes there, and you will hear.",
                short(&sha)
            ),
            Some(Unvouched::Unchecked { sha, on_branch: false }) => format!(
                "Not built yet: the project's check runs on the branch, and {} is only in your checkout. Land it with git_commit land true; Built passes by itself once the check passes on it, and you will hear.",
                short(&sha)
            ),
        };
        slice.done_said = true;
        card.updated_at = epoch_ms();
        let (passed, notices) = advance(board, id, Vec::new());
        state.woken.remove(agent);
        core.boards.save(&mut state, project);
        (passed, notices, note)
    };
    let notices = notices
        .into_iter()
        .filter(|notice| notice.to != agent)
        .collect();
    finish(core, project, id, &passed, notices);
    Ok(note)
}

fn short(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
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
        let id = params.card;
        let (passed, notices) = {
            let mut state = core.boards.state.locked();
            let board = state
                .boards
                .get_mut(&project)
                .ok_or_else(|| format!("There is no card #{id} on this project's board"))?;
            let card = board.card_mut(id)?;
            let label = card.label();
            let slice = card
                .slice
                .as_mut()
                .ok_or_else(|| format!("{label} has no gates to pass"))?;
            let waiting = slice.waiting();
            if waiting != Some(params.gate) {
                return Err(match waiting {
                    Some(gate) => format!(
                        "{label} waits at {}, not {}",
                        gate_words(gate),
                        gate_words(params.gate)
                    ),
                    None => format!("{label} passed every gate already"),
                }
                .into());
            }
            if slice.waits_on_person() != Some(params.gate) {
                return Err(format!(
                    "{} is not yours to pass on {label}",
                    gate_words(params.gate)
                )
                .into());
            }
            slice.passes.push(Pass {
                gate: params.gate,
                agent: None,
                at: epoch_ms(),
            });
            card.updated_at = epoch_ms();
            let moved = advance(board, id, vec![params.gate]);
            core.boards.save(&mut state, &project);
            moved
        };
        finish(&core, &project, id, &passed, notices);
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
            let board = state
                .boards
                .get_mut(&project)
                .ok_or_else(|| format!("There is no card #{id} on this project's board"))?;
            let card = board.card_mut(id)?;
            let label = card.label();
            let owner = card.owner_id().map(str::to_owned);
            let slice = card
                .slice
                .as_mut()
                .ok_or_else(|| format!("{label} is not a slice, so it takes no notes"))?;
            let on = match params.evidence {
                Some(evidence) => Some(
                    slice
                        .evidence(evidence)
                        .ok_or_else(|| format!("{label} has no evidence {evidence}"))?
                        .clone(),
                ),
                None => None,
            };
            if slice.notes.len() >= MAX_NOTES {
                slice.notes.remove(0);
            }
            slice.notes.push(Note {
                text: text.clone(),
                by: None,
                evidence: params.evidence,
                at: epoch_ms(),
            });
            let to = on.as_ref().and_then(|on| on.agent.clone()).or(owner);
            let about = match &on {
                Some(on) => format!(
                    "the {} it holds toward {}",
                    proof_words(&on.proof),
                    gate_words(on.gate)
                ),
                None => match slice.waiting() {
                    Some(gate) => format!("it, waiting at {}", gate_words(gate)),
                    None => "it".to_owned(),
                },
            };
            card.updated_at = epoch_ms();
            core.boards.save(&mut state, &project);
            (to, format!("On card {label}, about {about}:\n\n{text}"))
        };
        if let Some(to) = to {
            messages::from_person(&core, &to, message)?;
        }
        Ok(project_view(&core, &project))
    })
    .await
    .map_err(|error| error.to_string())
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
