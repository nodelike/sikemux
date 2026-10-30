//! Auto's blind review. Once a card's checks pass, an agent on another CLI
//! than its builder's reads the card, its acceptance criteria and the change,
//! in a read-only checkout of the pushed commit, and approves it or says what
//! to fix. It starts with nothing of the builder's, and no agent may reach it
//! or be reached by it. Each round gets a fresh one.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_process::sync::Locked;

use super::auto::Stage;
use super::finish::{self, change_run, Step};
use super::review::MAX_ROUNDS;
use super::{git, Card, State};
use crate::cli::protocol::{REVIEWER_OFFER, TOOLS_OFFER_ENV};
use crate::protocol::Event;
use crate::server::connection::blocking;
use crate::server::names::Starting;
use crate::server::spawn::Ending;
use crate::server::worktree;
use crate::server::{chat, messages, Core};

const MAX_DIFF_BYTES: usize = 200 * 1024;
const MAX_FINDINGS: usize = 50;
const MAX_FINDING_CHARS: usize = 4000;
const MAX_PATH_CHARS: usize = 1024;

/// The reviewer at work on a card's change.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Reviewer {
    pub agent: String,
    /// The commit it reviews.
    pub head: String,
    pub repo: String,
    /// Its read-only checkout, once made.
    #[serde(default)]
    pub checkout: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Finding {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
    pub text: String,
}

impl Finding {
    fn line(&self, number: usize) -> String {
        let place = match (&self.file, self.line) {
            (Some(file), Some(line)) => format!(" {file}:{line}"),
            (Some(file), None) => format!(" {file}"),
            _ => String::new(),
        };
        format!("{number}.{place} {}", self.text.trim())
    }
}

/// What a reviewer said of the commit it reviewed.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Verdict {
    pub head: String,
    pub approve: bool,
    #[serde(default)]
    pub findings: Vec<Finding>,
}

/// The card `agent_id` reviews for Auto.
pub(super) fn reviewed_card<'a>(
    state: &'a State,
    agent_id: &str,
) -> Option<(&'a String, &'a Card)> {
    state.boards.iter().find_map(|(project, board)| {
        board
            .cards
            .iter()
            .find(|card| reviews(card, agent_id))
            .map(|card| (project, card))
    })
}

fn reviews(card: &Card, agent_id: &str) -> bool {
    card.auto.as_ref().is_some_and(|run| {
        run.stage == Stage::Reviewing
            && run
                .reviewer
                .as_ref()
                .is_some_and(|reviewer| reviewer.agent == agent_id)
    })
}

/// What a reviewer is asked to do.
fn task(card: &Card, cwd: &Path, head: &str, from: &str, diff: &str, cut: bool) -> String {
    let short = |sha: &str| sha[..sha.len().min(10)].to_owned();
    let mut text = format!(
        "Sikemux's Auto started you to review a change blind: you know only what is below, and you cannot reach the agent that made it, or any other agent.\n\nCard #{}: {}",
        card.id, card.title
    );
    if !card.details.is_empty() {
        text.push_str(&format!("\n\n{}", card.details));
    }
    if card.acceptance.is_empty() {
        text.push_str(
            "\n\nThe person wrote no acceptance criteria; judge the change against the card.",
        );
    } else {
        text.push_str(&format!(
            "\n\nAcceptance criteria, written by the person:\n{}",
            card.acceptance
        ));
    }
    text.push_str(&format!(
        "\n\nYour folder, {}, is a read-only checkout of the change at {}. Read whatever you need there and run commands that change nothing; do not edit files, push, or comment on a pull request.\n\nThe change, from {} to {}:\n<diff>\n{diff}\n</diff>",
        cwd.display(),
        short(head),
        short(from),
        short(head),
    ));
    if cut {
        text.push_str(&format!(
            "\n\nThe diff is cut short here; git diff {from} {head} in your folder shows the rest."
        ));
    }
    text.push_str(
        "\n\nCheck that it does what the card asks and meets each acceptance criterion, and look for bugs, missing pieces and risks. Then call review_verdict once: approve true when it can merge as it is, or approve false with findings, each with the file, the line and what is wrong. Findings go to the agent that made the change, and a fresh reviewer looks again once it has fixed them. If you cannot judge without the person, ask them with ask and blocked true. End your turn once you have given your verdict.",
    );
    text
}

/// Has a reviewer look at `head` before it merges. With no agent on another
/// CLI to start, the pull request waits for the person.
pub(super) async fn begin(core: &Arc<Core>, step: &Step, head: &str) {
    let agent = format!("agent-{}", uuid::Uuid::new_v4().simple());
    let placed = change_run(core, &step.project, step.id, step.agent(), |_, run| {
        run.stage = Stage::Reviewing;
        run.verdict = None;
        run.note = Some("Starting an agent on another CLI to review it".to_owned());
        run.reviewer = Some(Reviewer {
            agent: agent.clone(),
            head: head.to_owned(),
            repo: run.repo.clone(),
            checkout: String::new(),
        });
    });
    if !placed {
        return;
    }
    match start(core, step, &agent, head).await {
        Ok(true) => {}
        Ok(false) => unseat(
            core,
            step,
            &agent,
            Stage::Review,
            "No signed-in agent on another CLI than its own can review it, so its pull request waits for you".to_owned(),
        ),
        Err(why) => unseat(
            core,
            step,
            &agent,
            Stage::Held,
            format!("Its reviewer did not start: {why}"),
        ),
    }
}

/// Takes the reviewer `agent` off the card's run, which moves to `stage`.
fn unseat(core: &Arc<Core>, step: &Step, agent: &str, stage: Stage, note: String) {
    let mut gone = None;
    change_run(core, &step.project, step.id, step.agent(), |_, run| {
        if run
            .reviewer
            .as_ref()
            .is_some_and(|reviewer| reviewer.agent == agent)
        {
            gone = run.reviewer.take();
        }
        run.stage = stage;
        run.note = Some(note);
    });
    if let Some(reviewer) = gone {
        dismiss_later(core, reviewer);
    }
}

/// Starts the reviewer. False when no agent can review the change.
async fn start(core: &Arc<Core>, step: &Step, agent: &str, head: &str) -> Result<bool, String> {
    let builder = super::cli_of(core, step.agent())
        .or_else(|| step.card.owner.as_ref().and_then(|owner| owner.cli.clone()));
    let chosen = step.settings.reviewer.clone();
    let Some(launcher) = core.workspaces.reviewer_launcher(
        chosen.as_ref().map(|chosen| chosen.launcher.as_str()),
        builder.as_deref(),
    )?
    else {
        return Ok(false);
    };
    let run = step.run.clone();
    let project = step.project.clone();
    let (repo, base) = (PathBuf::from(&run.repo), run.base.clone());
    let (head_sha, start_sha) = (head.to_owned(), run.start_sha.clone());
    let slug = agent.trim_start_matches("agent-")[..8].to_owned();
    let checkout = blocking(move || {
        let from = git::fork_point(&repo, &base, &head_sha).unwrap_or(start_sha);
        let (diff, cut) = git::diff(&repo, &from, &head_sha, MAX_DIFF_BYTES)?;
        let checkout = worktree::add_checkout(&repo, Path::new(&project), &head_sha, &slug)?;
        Ok((checkout, from, diff, cut))
    })
    .await
    .map_err(|error| error.to_string())?;
    let (checkout, from, diff, cut) = checkout;
    let path = checkout.path.to_string_lossy().into_owned();
    let recorded = change_run(core, &step.project, step.id, step.agent(), |_, run| {
        if let Some(reviewer) = run
            .reviewer
            .as_mut()
            .filter(|reviewer| reviewer.agent == agent)
        {
            reviewer.checkout = path.clone();
        }
    });
    if !recorded {
        let repo = PathBuf::from(&run.repo);
        let _ = blocking(move || {
            worktree::remove_checkout(&repo, &checkout.path);
            Ok(())
        })
        .await;
        return Ok(true);
    }
    let prepared = {
        let (core, agent, project, cwd) = (
            core.clone(),
            agent.to_owned(),
            step.project.clone(),
            checkout.cwd.clone(),
        );
        blocking(move || {
            let (provider, model) = chosen
                .map(|chosen| (chosen.provider, chosen.model))
                .unwrap_or_default();
            let (mut launch, launcher) =
                core.workspaces
                    .auto_launch(Some(&launcher), &cwd, provider.as_deref(), model)?;
            let (named, _) = core.names.give(
                Starting {
                    agent_id: &agent,
                    project,
                    provider: Some(&launch.provider),
                    resume_id: None,
                },
                core.open_agents(),
            );
            core.names.make_blind(&agent);
            launch.agent_id = agent.clone();
            launch
                .env
                .insert(TOOLS_OFFER_ENV.into(), REVIEWER_OFFER.into());
            Ok((launch, launcher, named.name))
        })
        .await
        .map_err(|error| error.to_string())?
    };
    let (launch, launcher, name) = prepared;
    let cli = launch.provider.clone();
    change_run(core, &step.project, step.id, step.agent(), |_, run| {
        run.note = Some(format!("{name}, on {cli}, reviews it blind"));
    });
    let text = task(&step.card, &checkout.cwd, head, &from, &diff, cut);
    let chat = chat::begin(core, launch, None, Some(launcher), false)
        .map_err(|error| error.to_string())?;
    core.broadcast_local(&Event::ChatBegun { chat: chat.info() });
    core.view_changed();
    chat::until_started(core, chat)
        .await
        .map_err(|error| error.to_string())?;
    messages::hand_task(core, agent, messages::auto_task(text)).await?;
    Ok(true)
}

/// Acts on the reviewer's verdict, or starts the review afresh when its
/// reviewer is gone without one, such as after Sikemux's background process
/// restarted.
pub(super) async fn settle(core: &Arc<Core>, step: Step) {
    let (Some(pull), Some(head)) = (step.run.pull.clone(), step.run.head.clone()) else {
        finish::ship(core, step.project.clone(), step.id);
        return;
    };
    let Some(verdict) = step.run.verdict.clone() else {
        if let Some(reviewer) = &step.run.reviewer {
            if core.running_agents().contains(&reviewer.agent) {
                return;
            }
        }
        let mut gone = None;
        change_run(core, &step.project, step.id, step.agent(), |_, run| {
            gone = run.reviewer.take();
        });
        if let Some(reviewer) = gone {
            dismiss(core, reviewer).await;
        }
        return begin(core, &step, &head).await;
    };
    let mut gone = None;
    change_run(core, &step.project, step.id, step.agent(), |_, run| {
        gone = run.reviewer.take();
        run.verdict = None;
        if verdict.approve && verdict.head == head {
            run.approved = Some(head.clone());
        }
    });
    if let Some(reviewer) = gone {
        dismiss(core, reviewer).await;
    }
    if verdict.head != head {
        return begin(core, &step, &head).await;
    }
    if verdict.approve {
        return finish::merge(core, &step, &pull, &head).await;
    }
    let findings: Vec<String> = verdict
        .findings
        .iter()
        .enumerate()
        .map(|(index, finding)| finding.line(index + 1))
        .collect();
    let rounds = step.run.review.rounds;
    if rounds >= MAX_ROUNDS {
        step.hold(
            core,
            format!(
                "A reviewer still asked for changes after {} fixed {MAX_ROUNDS} rounds of them:\n{}",
                step.name(core),
                findings.join("\n")
            ),
        );
        return;
    }
    let text = format!(
        "An agent on another CLI reviewed your change blind, against the card and its acceptance criteria, and asks for these changes before it merges:\n\n{}\n\nFix them on {}, run the checks that cover them and commit. When your turn ends, Sikemux pushes, checks again and has a fresh reviewer look at the new change. This is round {} of {MAX_ROUNDS}.",
        findings.join("\n"),
        step.run.branch,
        rounds + 1
    );
    let note = format!(
        "A reviewer asked for {} change{}, so it is fixing them",
        findings.len(),
        if findings.len() == 1 { "" } else { "s" }
    );
    finish::ask(core, &step, text, &note, |run| run.review.rounds += 1).await;
}

/// Keeps the verdict the reviewer `agent_id` gave, for Auto to act on.
pub(super) fn record(core: &Core, agent_id: &str, params: &Value) -> Result<Value, String> {
    let approve = params
        .get("approve")
        .and_then(Value::as_bool)
        .ok_or("approve must be true or false")?;
    let findings = findings(params.get("findings"))?;
    if !approve && findings.is_empty() {
        return Err("Say what must change in findings when you do not approve".into());
    }
    let mut state = core.boards.state.locked();
    let (project, card) = state
        .boards
        .iter_mut()
        .flat_map(|(project, board)| board.cards.iter_mut().map(move |card| (project, card)))
        .find(|(_, card)| reviews(card, agent_id))
        .ok_or("review_verdict is only for a reviewer Auto started, while its review is open")?;
    let run = card.auto.as_mut().ok_or("the review is over")?;
    if run.verdict.is_some() {
        return Err("You gave your verdict already; end your turn".into());
    }
    let head = run
        .reviewer
        .as_ref()
        .map(|reviewer| reviewer.head.clone())
        .unwrap_or_default();
    run.note = Some(if approve {
        "Its reviewer approved it".to_owned()
    } else {
        format!(
            "Its reviewer asked for {} change{}",
            findings.len(),
            if findings.len() == 1 { "" } else { "s" }
        )
    });
    run.verdict = Some(Verdict {
        head,
        approve,
        findings,
    });
    card.updated_at = sikemux_process::clock::epoch_ms();
    let project = project.clone();
    core.boards.save(&mut state, &project);
    Ok(json!({
        "recorded": true,
        "note": "End your turn now; Auto takes it from here.",
    }))
}

fn findings(value: Option<&Value>) -> Result<Vec<Finding>, String> {
    let list = match value {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(list)) if list.len() <= MAX_FINDINGS => list,
        Some(_) => return Err(format!("findings must be a list of at most {MAX_FINDINGS}")),
    };
    list.iter()
        .map(|item| {
            let text = item
                .get("text")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty() && text.chars().count() <= MAX_FINDING_CHARS)
                .ok_or(format!(
                    "each finding needs its text, at most {MAX_FINDING_CHARS} characters"
                ))?;
            let file = match item.get("file") {
                None | Some(Value::Null) => None,
                Some(Value::String(file)) if file.chars().count() <= MAX_PATH_CHARS => {
                    Some(file.trim().to_owned()).filter(|file| !file.is_empty())
                }
                Some(_) => return Err("a finding's file must be a path".into()),
            };
            let line = match item.get("line") {
                None | Some(Value::Null) => None,
                Some(line) => Some(line.as_u64().ok_or("a finding's line must be a number")?),
            };
            Ok(Finding {
                file,
                line,
                text: text.to_owned(),
            })
        })
        .collect()
}

/// The reviewer's turn ended. False when `agent_id` reviews no card. One
/// that ended without a verdict, and waits on no answer, gives the card back
/// to the person.
pub(crate) fn turn_ended(core: &Arc<Core>, agent_id: &str, ending: &Ending) -> bool {
    let found = {
        let state = core.boards.state.locked();
        reviewed_card(&state, agent_id).and_then(|(project, card)| {
            let run = card.auto.as_ref()?;
            Some((
                project.clone(),
                card.id,
                run.agent.clone(),
                run.verdict.is_some(),
            ))
        })
    };
    let Some((project, id, builder, judged)) = found else {
        return false;
    };
    if judged {
        return true;
    }
    let name = core
        .names
        .name(agent_id)
        .unwrap_or_else(|| "Its reviewer".to_owned());
    let why = match ending {
        Ending::Finished if core.harness.questions.waits_on_answer(agent_id) => return true,
        Ending::Finished => format!("{name} ended its review without a verdict"),
        Ending::Stopped => format!("{name} was stopped before it gave a verdict"),
        Ending::Failed(reason) => format!("{name}'s review failed: {reason}"),
        Ending::Gone { reason, .. } => match reason {
            Some(reason) if !reason.trim().is_empty() => {
                format!("{name} stopped running before it gave a verdict: {reason}")
            }
            _ => format!("{name} stopped running before it gave a verdict"),
        },
    };
    finish::hold(core, &project, id, &builder, why);
    true
}

/// Stops a reviewer and takes its checkout away once Auto's next look runs.
pub(super) fn dismiss_later(core: &Core, reviewer: Reviewer) {
    core.boards.state.locked().dismissed.push(reviewer);
    core.boards.wake.now();
}

pub(super) fn clear_dismissed(core: &Arc<Core>) {
    let dismissed = std::mem::take(&mut core.boards.state.locked().dismissed);
    for reviewer in dismissed {
        let core = core.clone();
        tokio::spawn(async move { dismiss(&core, reviewer).await });
    }
}

pub(super) async fn dismiss(core: &Arc<Core>, reviewer: Reviewer) {
    if core.chats.stop(&reviewer.agent) {
        core.note_agent_gone(&reviewer.agent);
    }
    if reviewer.checkout.is_empty() {
        return;
    }
    let (repo, path) = (
        PathBuf::from(reviewer.repo),
        PathBuf::from(reviewer.checkout),
    );
    let _ = blocking(move || {
        worktree::remove_checkout(&repo, &path);
        Ok(())
    })
    .await;
}
