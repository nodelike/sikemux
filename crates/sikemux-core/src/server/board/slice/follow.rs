//! How the board follows a project's commits, landings and checks, in the order they happened, and what it does to the slices they are about.

use super::*;
use crate::server::changed::ChangedCore;
use crate::server::parts::Parts;
use crate::server::repo_state::RepoParts;

/// The card `agent` works now on `board`, when that is a slice in progress.
pub(crate) fn working_slice(board: &Board, agent: &str) -> Option<u64> {
    let card = board.working_card(agent)?;
    let slice = match &card.slice {
        Some(_) => card,
        None => (board.cards.iter())
            .find(|slice| (slice.slice.as_ref()).is_some_and(|slice| slice.fix == Some(card.id)))?,
    };
    (slice.status == CardStatus::Progress && slice.owner_id() == Some(agent)).then_some(slice.id)
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
pub(crate) fn rebind(board: &mut Board) -> Vec<u64> {
    let goals: Vec<(u64, u64)> = (board.cards.iter())
        .filter(|card| card.slice.is_some())
        .map(|card| (card.id, board.goal_above(card).id))
        .collect();
    let mut moved = Vec::new();
    for (id, goal) in goals {
        let standing: Vec<Named> = (board.card(goal).ok())
            .map(|card| {
                (card.goal.assertions().iter())
                    .filter(|assertion| assertion.retired.is_none())
                    .map(|assertion| Named::of(goal, assertion, id))
                    .collect()
            })
            .unwrap_or_default();
        let Some(slice) = board.card_mut(id).ok().and_then(|card| card.slice.as_mut()) else {
            continue;
        };
        let freed = slice.bind(goal, |id| {
            (standing.iter()).find(|named| named.id == id).cloned()
        });
        if freed {
            moved.push(id);
        }
    }
    moved
}

/// The project's check failed on `sha`, the latest commit of the slice on
/// `id`: the fix goes on a card under it, held by its owner, which the slice
/// waits on. A slice keeps one open fix card, which a later failure goes to,
/// and one fix card for each commit. What the owner is told of it.
pub(crate) fn file_fix(core: &Core, board: &mut Board, id: u64, sha: &str) -> Option<String> {
    let slice = board.card(id).ok()?;
    let (owner, label) = (slice.owner_id()?.to_owned(), slice.label());
    let title = format!("Fix: the project's check failed on {}", short(sha));
    let filed = (slice.slice.as_ref()?.fix).and_then(|fix| board.card(fix).ok());
    let open = (filed.filter(|fix| fix.status != CardStatus::Done)).map(|fix| fix.id);
    if let Some(done) = filed.filter(|fix| fix.status == CardStatus::Done && fix.title == title) {
        return Some(format!(
            "#{} was this commit's fix and is done, yet the check failed on it again: no new card is filed for it. Mend it with a new commit, such as one rebased on the project's branch, then mark {label} done again.",
            done.id
        ));
    }
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
    if board.cards.len() >= super::super::MAX_CARDS {
        return None;
    }
    let settings = board.settings.clone();
    let fix = board.next.max(1);
    board.next = fix + 1;
    let card = super::super::new_card(
        core,
        fix,
        super::super::Adding {
            title,
            details: format!("{failed} Mend it here; {label} waits on this card, and once it is done, mark {label} done again."),
            parent: Some(id),
            ..super::super::Adding::default()
        },
        None,
        &settings,
    );
    board.cards.push(card);
    super::super::take_on(core, board, fix, &owner).ok()?;
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
        .boards()
        .state
        .locked()
        .boards
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    core.repo_events().lost(every);
    let mut panics = 0;
    loop {
        let told = core.repo_events().next().await;
        let projects: BTreeSet<String> = told
            .events
            .iter()
            .map(|event| match event {
                RepoEvent::Committed { project, .. }
                | RepoEvent::Landed { project, .. }
                | RepoEvent::Deployed { project, .. }
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
                core.repo_events().lost(projects);
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
        let state = core.boards().state.locked();
        let Some(board) = state.boards.get(project) else {
            return;
        };
        board
            .cards
            .iter()
            .filter(|card| card.status == CardStatus::Progress)
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
    if let RepoEvent::Committed { agent, .. } = &event {
        super::super::chat_goal::committed(core, agent);
        (core.board_above().inbox_committed)(core, agent);
    }
    if let RepoEvent::Committed { agent, .. } | RepoEvent::Landed { agent, .. } = &event {
        (core.board_above().turn_state_read_work)(core, agent);
    }
    if let RepoEvent::Checked {
        project,
        covered,
        verdict: Some(Verdict::Passed),
        ..
    } = &event
    {
        for (agent, turn, head) in (core.board_above().inbox_showing)(core, project, covered) {
            (core.board_above().pictures_take)(core, &agent, project, turn, head);
        }
    }
    if let RepoEvent::Deployed {
        project,
        agent,
        ran,
    } = &event
    {
        let passed = ran.verdict == Some(Verdict::Passed);
        if let (true, Some(head)) = (
            passed && !crate::server::board::is_production(core, project, &ran.environment),
            ran.head.clone(),
        ) {
            (core.board_above().inbox_staged)(core, agent, head);
            core.changed(crate::server::changed::What::View);
        }
    }
    let known = match &event {
        RepoEvent::Committed { project, agent, .. } | RepoEvent::Landed { project, agent, .. } => {
            let state = core.boards().state.locked();
            state.boards.get(project).and_then(|board| {
                let card = board.working_card(agent)?;
                card.slice.as_ref()?.head().map(str::to_owned)
            })
        }
        RepoEvent::Deployed { .. } | RepoEvent::CheckStarted { .. } | RepoEvent::Checked { .. } => {
            None
        }
    };
    let ancestor = |project: &str, older: &str, newer: &str| {
        Git::quick(Path::new(project))
            .trimmed(&["merge-base", "--is-ancestor", older, newer])
            .is_ok()
    };
    let from = match &event {
        RepoEvent::Committed { project, sha, .. } => Git::quick(Path::new(project))
            .trimmed(&["rev-parse", &format!("{sha}^")])
            .unwrap_or_else(|_| sha.clone()),
        // Nothing reached the branch since the slice's commits were made on it, so the landed
        // ones carry on the slice's run rather than starting one of their own.
        RepoEvent::Landed { project, onto, .. } => match &known {
            Some(known) if ancestor(project, onto, known) => known.clone(),
            _ => onto.clone(),
        },
        RepoEvent::Deployed { .. } | RepoEvent::CheckStarted { .. } | RepoEvent::Checked { .. } => {
            String::new()
        }
    };
    let older = match &event {
        RepoEvent::Committed { project, sha, .. }
        | RepoEvent::Landed {
            project, tip: sha, ..
        } => known.filter(|known| known != sha && ancestor(project, sha, known)),
        RepoEvent::Deployed { .. } | RepoEvent::CheckStarted { .. } | RepoEvent::Checked { .. } => {
            None
        }
    };
    let behind = |card: &Card| {
        older.is_some() && card.slice.as_ref().and_then(Slice::head) == older.as_deref()
    };
    let mut notices = Vec::new();
    let (project, touched) = {
        let mut state = core.boards().state.locked();
        let State { boards, runs, .. } = &mut *state;
        let mut moved = |project: &str, agent: &str, head: Head, new_work: bool| {
            let board = boards.get_mut(project)?;
            move_work(board, runs.get(project), agent, &head, new_work, behind)
        };
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
                let Some(id) = moved(&project, &agent, head, true) else {
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
                let Some(id) = moved(&project, &agent, head, false) else {
                    return;
                };
                (project, vec![id])
            }
            RepoEvent::Deployed {
                project,
                agent,
                ran,
            } => {
                drop(state);
                return deployed(core, &project, &agent, ran);
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
                core.boards().wake.now();
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
                        .filter(|card| card.status != CardStatus::Done && card.slice.is_some())
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
                        let fixed = fix_for(core, board, id, &refused);
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
        .map(|id| (id, seen(core, &project, id, &Err(NoCheck::Unnamed))))
        .collect();
    let mut later = Later::default();
    {
        let mut state = core.boards().state.locked();
        for (id, seen) in seen {
            match super::super::advance(core, &mut state, &project, &By::Auto("Board"), id, &seen) {
                Ok((sent, more)) => {
                    notices.extend(sent);
                    later = later.and(more);
                }
                Err(error) => eprintln!("sikemux core: the board did not follow #{id}: {error}"),
            }
        }
        core.boards().save(&mut state, &project);
    }
    later.run(core);
    send_notices(core, notices);
}
