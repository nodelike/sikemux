//! Assertions a command shows. Once the person approved the command, the board
//! runs it on a slice's latest commit in a clean checkout, as the project's
//! check runs, and keeps what it said as each such assertion's evidence. Its
//! exit code is the verdict. A new commit runs it again, and so does the owner
//! marking the card done again; a verdict stands until then. It does not run on
//! a commit that holds a repository file it names differently from what the
//! person accepted, which waits for them to answer: a plain path in the command
//! is all this can see, so what the command calls can still change.

use crate::server::changed::ChangedCore;
use crate::server::parts::Parts;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sikemux_process::sync::Locked;
use sikemux_view::slice::{NamedFile, Verdict};

use super::slice::{moved_by_evidence, short};
use super::{CardStatus, Notice, State};
use crate::git::Git;
use crate::server::changed::What;
use crate::server::Core;

/// Commands running at once, across projects. Each project runs one at a time,
/// as its one checkout for them can hold only one commit.
const MAX_RUNNING: usize = 2;
/// Words of a command looked up as files.
const MAX_NAMED: usize = 64;

/// A card, its latest commit, and a command that has not passed there.
type Waiting = (u64, String, String);

#[derive(Default)]
pub(super) struct Runs {
    /// The commit and command running now in each project running one.
    running: HashMap<String, (String, String)>,
    /// What each project already ran or is running.
    ran: HashMap<String, BTreeSet<Waiting>>,
}

/// The words of `command` that could be a path inside the project.
fn names_in(command: &str) -> Vec<String> {
    let mut names: Vec<String> = command
        .split(|at: char| at.is_whitespace() || ";|&()<>'\"`=,".contains(at))
        .map(|word| {
            let mut word = word;
            while let Some(rest) = word.strip_prefix("./") {
                word = rest;
            }
            word
        })
        .filter(|word| {
            !word.is_empty()
                && !word.starts_with(['-', '/', '~'])
                && !word.contains('$')
                && !word.split('/').any(|part| part == "." || part == "..")
        })
        .map(str::to_owned)
        .collect();
    names.sort();
    names.dedup();
    names.truncate(MAX_NAMED);
    names
}

/// Each word of `command` that could be a path inside `project`, as `commit` holds it. One that
/// is a folder, or nothing, it does not hold as a file.
pub(super) fn named_at(
    project: &Path,
    commit: &str,
    command: &str,
) -> Result<Vec<NamedFile>, String> {
    let names = names_in(command);
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let mut args = vec!["--literal-pathspecs", "ls-tree", "-z", commit, "--"];
    args.extend(names.iter().map(String::as_str));
    let listed = Git::quick(project).trimmed(&args)?;
    let blobs: HashMap<&str, &str> = listed
        .split('\0')
        .filter_map(|entry| {
            let (about, path) = entry.split_once('\t')?;
            let mut about = about.split(' ');
            let (kind, id) = (about.nth(1)?, about.next()?);
            (kind == "blob").then_some((path, id))
        })
        .collect();
    Ok(names
        .iter()
        .map(|name| NamedFile {
            path: name.clone(),
            blob: blobs.get(name.as_str()).map(|blob| (*blob).to_owned()),
        })
        .collect())
}

/// What the person has said of `named` on the slices at `head` that wait on `command`.
#[derive(Default)]
struct Looked {
    /// Assertions it may run for.
    clear: bool,
    /// Those they turned down.
    refused: Vec<String>,
}

/// Compares `named` with what the person accepted for each assertion `command` shows on the slices
/// at `head`. What differs waits for them, and those slices' owners hear.
fn compare_files(
    core: &Arc<Core>,
    project: &str,
    head: &str,
    command: &str,
    named: &[NamedFile],
) -> Looked {
    let mut looked = Looked::default();
    let mut notices = Vec::new();
    let mut state = core.boards().state.locked();
    let State {
        boards, commands, ..
    } = &mut *state;
    let Some(board) = boards.get_mut(project) else {
        return looked;
    };
    let slices: Vec<_> = (board.cards.iter())
        .filter_map(|card| {
            let slice = card
                .slice
                .as_ref()
                .filter(|slice| slice.head() == Some(head))?;
            Some((
                card.id,
                card.owner_id().map(str::to_owned),
                card.label(),
                slice.commanded_by(command),
            ))
        })
        .collect();
    for (card, owner, label, assertions) in slices {
        let mut asked: Vec<String> = Vec::new();
        for (goal, id) in assertions {
            let Ok(goal) = board.card_mut(goal) else {
                continue;
            };
            let (waits, refused) = goal.goal.files_of_slice(id, card, head, named);
            looked.clear |= waits.is_empty() && refused.is_empty();
            asked.extend(waits.into_iter().map(|file| file.path));
            looked
                .refused
                .extend(refused.into_iter().map(|file| file.path));
        }
        if asked.is_empty() {
            continue;
        }
        asked.sort();
        asked.dedup();
        if let Some(ran) = commands.ran.get_mut(project) {
            ran.remove(&(card, head.to_owned(), command.to_owned()));
        }
        notices.extend(owner.map(|owner| Notice {
            to: owner,
            text: format!(
                "Your card {label}: `{command}` did not run on {}, as {} differs from what the person accepted. It waits for them, which only they can answer, and runs if they accept it.",
                short(head),
                asked.join(", ")
            ),
        }));
    }
    super::slice::rebind(board);
    core.boards().save(&mut state, project);
    drop(state);
    core.changed(What::View);
    super::send_notices(core, notices);
    looked
}

fn waiting(state: &State) -> HashMap<String, BTreeSet<Waiting>> {
    let mut found: HashMap<String, BTreeSet<Waiting>> = HashMap::new();
    for (project, board) in &state.boards {
        for card in board
            .cards
            .iter()
            .filter(|card| card.status == CardStatus::Progress)
        {
            let Some(slice) = &card.slice else {
                continue;
            };
            let Some(head) = slice.head() else {
                continue;
            };
            for command in slice.awaiting_commands() {
                found.entry(project.clone()).or_default().insert((
                    card.id,
                    head.to_owned(),
                    command,
                ));
            }
        }
    }
    found
}

/// Starts the command next in line in each project with none running, as far as the limit allows.
pub(super) fn due(core: &Arc<Core>) {
    let mut starts = Vec::new();
    {
        let mut state = core.boards().state.locked();
        let forgot: Vec<String> = (state.boards.iter_mut())
            .filter_map(|(project, board)| {
                super::goal::forget_moved_on(board).then(|| project.clone())
            })
            .collect();
        let any_forgotten = !forgot.is_empty();
        for project in forgot {
            core.boards().save(&mut state, &project);
        }
        if any_forgotten {
            core.changed(What::View);
        }
        let waiting = waiting(&state);
        let runs = &mut state.commands;
        runs.ran.retain(|project, _| waiting.contains_key(project));
        for (project, slices) in waiting {
            let ran = runs.ran.entry(project.clone()).or_default();
            ran.retain(|one| slices.contains(one));
            if runs.running.contains_key(&project) || runs.running.len() >= MAX_RUNNING {
                continue;
            }
            let Some((_, head, command)) = slices.iter().find(|one| !ran.contains(*one)) else {
                continue;
            };
            let (head, command) = (head.clone(), command.clone());
            ran.extend(
                (slices.iter())
                    .filter(|(_, at, wanted)| *at == head && *wanted == command)
                    .cloned(),
            );
            (runs.running).insert(project.clone(), (head.clone(), command.clone()));
            starts.push((project, head, command));
        }
    }
    for (project, head, command) in starts {
        tokio::spawn(run(core.clone(), project, head, command));
    }
}

async fn run(core: Arc<Core>, project: String, head: String, command: String) {
    let reading = {
        let (dir, head, command) = (PathBuf::from(&project), head.clone(), command.clone());
        tokio::task::spawn_blocking(move || named_at(&dir, &head, &command))
            .await
            .unwrap_or_else(|error| Err(error.to_string()))
    };
    let outcome = match reading {
        Err(why) => Some(Err(format!("the files it names could not be read: {why}"))),
        Ok(named) => {
            let looked = compare_files(&core, &project, &head, &command, &named);
            match (looked.clear, looked.refused.as_slice()) {
                (_, [_, ..]) => Some(Err(format!(
                    "The person turned down {} as this commit holds it, so it did not run. Put back what they accepted",
                    looked.refused.join(", ")
                ))),
                (false, []) => None,
                (true, []) => Some(run_in_checkout(&core, &project, &head, &command).await),
            }
        }
    };
    if let Some(outcome) = outcome {
        let (verdict, output) = match outcome {
            Ok((true, output)) => (Some(Verdict::Passed), output),
            Ok((false, output)) => (Some(Verdict::Failed), output),
            Err(why) => (None, why),
        };
        keep(&core, &project, &head, &command, verdict, output);
    }
    core.boards()
        .state
        .locked()
        .commands
        .running
        .remove(&project);
    core.boards().wake.now();
}

async fn run_in_checkout(
    core: &Arc<Core>,
    project: &str,
    head: &str,
    command: &str,
) -> Result<(bool, String), String> {
    let checkout = crate::server::worktree::agent::gates(core, Path::new(project), head)
        .await
        .map_err(|error| format!("its checkout is not ready: {error}"))?;
    let command = command.to_owned();
    tokio::task::spawn_blocking(move || crate::server::checks::run_command(&checkout, &command))
        .await
        .unwrap_or_else(|error| Err(error.to_string()))
}

/// Each slice still at `head` that waits on `command` keeps the run, and its owner hears when it
/// did not pass.
fn keep(
    core: &Arc<Core>,
    project: &str,
    head: &str,
    command: &str,
    verdict: Option<Verdict>,
    output: String,
) {
    let cards: Vec<(u64, Option<String>, String)> = {
        let state = core.boards().state.locked();
        (state.boards.get(project).into_iter())
            .flat_map(|board| board.cards.iter())
            .filter(|card| {
                (card.slice.as_ref()).is_some_and(|slice| {
                    slice.head() == Some(head)
                        && slice.awaiting_commands().iter().any(|one| one == command)
                })
            })
            .map(|card| (card.id, card.owner_id().map(str::to_owned), card.label()))
            .collect()
    };
    let mut notices = Vec::new();
    for (card, owner, label) in cards {
        let mut assertions = Vec::new();
        moved_by_evidence(core, project, card, |slice| {
            assertions =
                slice.command_ran(owner.as_deref(), head, command, verdict, output.clone());
        });
        let Some(owner) = owner.filter(|_| !assertions.is_empty()) else {
            continue;
        };
        let at = short(head);
        let then = match verdict {
            Some(Verdict::Passed) => continue,
            Some(Verdict::Failed) => format!(
                "failed on {at}. It runs again on your next commit, or when you mark the card done again"
            ),
            None => format!(
                "gave no verdict on {at}. Mark the card done again to run it again"
            ),
        };
        notices.push(Notice {
            to: owner,
            text: format!(
                "Your card {label}: `{command}`, which assertion {} needs, {then}.\n\n{output}",
                assertions
                    .iter()
                    .map(|id| format!("#{id}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        });
    }
    super::send_notices(core, notices);
}

/// The owner of slice `card` marked it done again, so what did not pass on it runs again, other
/// than what is running now.
pub(super) fn run_again(core: &Core, project: &str, card: u64) {
    let mut state = core.boards().state.locked();
    let runs = &mut state.commands;
    let running = runs.running.get(project).cloned();
    if let Some(ran) = runs.ran.get_mut(project) {
        ran.retain(|(of, head, command)| {
            *of != card
                || running
                    .as_ref()
                    .is_some_and(|(at, wanted)| at == head && wanted == command)
        });
    }
    drop(state);
    core.boards().wake.now();
}
