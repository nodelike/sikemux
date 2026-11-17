//! The person's asks: one for each message they send an agent, kept as a task on its project's
//! board from then until its change is in the app they run. The agents the receiving agent starts
//! or messages in that turn take it over. A chief's turn that hands it to nobody answered it, as
//! did another agent's turn that left its work where it was, and an answered ask leaves the board.
//! An ask outlives the agents on it: once every one has closed before its work reached the
//! project's branch, nobody has it. An agent closes one handled without a hand-off with
//! `task_update`'s `asked`.

use super::*;
use crate::cli::protocol::HarnessRequest;
use crate::server::board::asked::{Asked, Holder, Stood};
use crate::server::board::{Board, Card};
use crate::server::board_state::BoardParts;
use crate::server::chat_state::ChatParts;
use crate::server::chief;
use crate::text::{clipped, clipped_line};
use serde_json::{json, Value};
use sikemux_markdown::paths::is_image_path;
use sikemux_view::goals::{AgentDesk, DeskPage};
use sikemux_view::now::{AskCall, AskSaid, AskStand, NowAsk};
use std::collections::{BTreeMap, BTreeSet};

const TITLE_CHARS: usize = 160;
/// How long an ask stays on the Now card once its change is in the app the person runs.
const SHOWN_IN_BUILD_MS: u64 = 24 * 60 * 60 * 1000;
/// The same words to the same agent within this long are one ask, sent again.
const SAME_ASK_MS: u64 = 10 * 60 * 1000;
/// The most pages, and the most files, an ask keeps of what was opened while it was answered.
const MOST_OPENED: usize = 24;
/// The most the person sent under one ask that it keeps, the ask itself and the latest.
const MOST_SAID: usize = 24;
const SAID_CHARS: usize = 4000;

/// An ask as an earlier core kept it in `now.json`, which moves onto its project's board.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Carried {
    project: String,
    title: String,
    at: u64,
    #[serde(flatten)]
    asked: Asked,
}

/// Puts the asks an earlier core kept in `now.json` on their projects' boards, once a copy of
/// that file as it was is kept beside it.
pub(super) fn carry_over(core: &Core) {
    if core.now().state.locked().asks.is_empty() {
        return;
    }
    if let Err(error) = core.now().kept.copy_once("before-tasks") {
        eprintln!("sikemux core: the asks stay in now.json, as it could not be copied: {error}");
        return;
    }
    let carried = std::mem::take(&mut core.now().state.locked().asks);
    let boards = core.boards();
    let mut state = boards.state.locked();
    let mut folders = BTreeSet::new();
    for ask in carried {
        let Some(folder) = folder_of(core, &ask.project) else {
            continue;
        };
        let board = state.made(&folder);
        let moved_before =
            asks(board).any(|(card, asked)| card.at() == ask.at && asked.to == ask.asked.to);
        if !moved_before {
            board.ask(ask.title, ask.at, ask.asked);
            folders.insert(folder);
        }
    }
    for folder in folders {
        boards.save(&mut state, &folder);
    }
    drop(state);
    let now = core.now();
    now.kept.save(&*now.state.locked());
    core.changed(What::View);
}

/// The folder of the workspace project `project`.
fn folder_of(core: &Core, project: &str) -> Option<String> {
    (core.workspaces().projects().into_iter())
        .find(|it| it.id == project)
        .map(|it| it.path.to_string_lossy().into_owned())
}

/// The folder of the project `agent` works in.
fn folder_of_agent(core: &Core, agent: &str) -> Option<String> {
    folder_of(core, &core.workspaces().agent(agent)?.project)
}

fn stood(core: &Core, agent: &str) -> Option<Stood> {
    let work = core.inbox().ending(agent)?.work?;
    Some(Stood {
        changes: work.changes,
        head: work.head,
    })
}

fn holder(core: &Core, agent: &str) -> Holder {
    Holder {
        agent: agent.to_owned(),
        before: stood(core, agent),
    }
}

/// Whether `holder`'s work, which stands at `now`, moved since it took the ask.
fn moved(now: Option<&Stood>, holder: &Holder) -> bool {
    now.is_some_and(|now| now.changes != WorkAt::None && holder.before.as_ref() != Some(now))
}

/// Changes the asks of every board, saving each board `change` says it changed.
fn change(core: &Core, mut change: impl FnMut(&mut Board, u64) -> bool) {
    let at = epoch_ms();
    let boards = core.boards();
    let mut state = boards.state.locked();
    let changed: Vec<String> = (state.boards.values_mut())
        .filter_map(|board| change(board, at).then(|| board.folder().to_owned()))
        .collect();
    for folder in &changed {
        boards.save(&mut state, folder);
    }
    drop(state);
    if !changed.is_empty() {
        core.changed(What::View);
    }
}

/// Changes the asks of the board of the project at `folder`, made when it has none.
fn change_in(core: &Core, folder: &str, change: impl FnOnce(&mut Board, u64) -> bool) {
    let at = epoch_ms();
    let boards = core.boards();
    let mut state = boards.state.locked();
    if !change(state.made(folder), at) {
        return;
    }
    boards.save(&mut state, folder);
    drop(state);
    core.changed(What::View);
}

/// Changes what each ask on `board` keeps; whether any changed.
fn each_ask(board: &mut Board, at: u64, mut change: impl FnMut(u64, &mut Asked) -> bool) -> bool {
    let mut changed = false;
    for card in &mut board.cards {
        let id = card.id;
        changed |= card
            .change_asked(at, |asked| change(id, asked))
            .unwrap_or(false);
    }
    changed
}

fn asks(board: &Board) -> impl Iterator<Item = (&Card, &Asked)> {
    (board.cards.iter()).filter_map(|card| Some((card, card.asked()?)))
}

/// The title of an ask the person sent as `text` with the files at `paths`.
pub(crate) fn title(text: &str, paths: &[String]) -> String {
    clipped_line(&sikemux_fold::person_wrote(text, paths), TITLE_CHARS)
}

fn images(paths: &[String]) -> Vec<String> {
    (paths.iter())
        .filter(|path| is_image_path(path))
        .cloned()
        .collect()
}

/// The person sent `text` to `agent`, with the files at `paths`, as a reply on the ask `filed`
/// when it names one. A reply, and an answer to a call of an agent on an ask, go under that ask.
/// The ask it is, by number.
pub(crate) fn asked(
    core: &Core,
    agent: &str,
    text: &str,
    paths: &[String],
    filed: Option<u64>,
) -> Option<u64> {
    let at = epoch_ms();
    if let Some(answer) = sikemux_fold::call_answered(text, paths) {
        let asker = Some(answer.agent_id.as_str())
            .filter(|asker| !asker.is_empty())
            .unwrap_or(agent);
        let said = AskSaid {
            at,
            images: images(&answer.attachments),
            text: clipped(&answer.text, SAID_CHARS),
            call: Some(AskCall {
                asker: asker.to_owned(),
                question: clipped(&answer.question, SAID_CHARS),
            }),
        };
        return said_under(core, filed, asker, said);
    }
    if filed.is_some() {
        let said = AskSaid {
            at,
            images: images(paths),
            text: clipped(&sikemux_fold::person_wrote(text, paths), SAID_CHARS),
            call: None,
        };
        return said_under(core, filed, agent, said);
    }
    let title = title(text, paths);
    let images = images(paths);
    let folder = (!title.is_empty() || !images.is_empty())
        .then(|| folder_of_agent(core, agent))
        .flatten()?;
    let said = AskSaid {
        at,
        text: clipped(&sikemux_fold::person_wrote(text, paths), SAID_CHARS),
        images: images.clone(),
        call: None,
    };
    let to_before = stood(core, agent);
    let mut id = None;
    change_in(core, &folder, |board, at| {
        let again = asks(board).find(|(card, asked)| {
            asked.to == agent
                && card.title() == title
                && asked.images == images
                && at.saturating_sub(card.at()) < SAME_ASK_MS
        });
        if let Some((again, _)) = again {
            id = Some(again.id);
            return false;
        }
        let asked = Asked {
            to: agent.to_owned(),
            images,
            said: vec![said],
            to_before,
            in_turn: true,
            ..Asked::default()
        };
        id = Some(board.ask(title, at, asked));
        true
    });
    id
}

/// Puts `said` under the ask `filed` of `agent`'s project, else the newest one `agent` is on that
/// is not yet in the person's build. The ask it went under.
fn said_under(core: &Core, filed: Option<u64>, agent: &str, said: AskSaid) -> Option<u64> {
    let on = |asked: &Asked| {
        asked.in_build_at.is_none()
            && (asked.to == agent || asked.owners.iter().any(|holder| holder.agent == agent))
    };
    let place = match filed {
        Some(filed) => folder_of_agent(core, agent).map(|folder| (folder, filed)),
        None => (core.boards().state.locked().boards.values())
            .flat_map(|board| asks(board).map(move |(card, asked)| (board, card, asked)))
            .filter(|(_, _, asked)| on(asked))
            .max_by_key(|(_, card, _)| card.at())
            .map(|(board, card, _)| (board.folder().to_owned(), card.id)),
    };
    let (folder, id) = place?;
    let mut went = None;
    change_in(core, &folder, |board, at| {
        let Ok(card) = board.card_mut(id) else {
            return false;
        };
        let pushed = card.change_asked(at, |asked| {
            asked.said.push(said);
            if asked.said.len() > MOST_SAID {
                asked.said.remove(1);
            }
        });
        went = pushed.map(|()| id);
        went.is_some()
    });
    went.or(filed)
}

/// What the person sent under the ask `id` of the workspace project `project`, the ask first.
pub(crate) fn said(core: &Core, folder: &str, id: u64) -> Vec<AskSaid> {
    let state = core.boards().state.locked();
    (state.of(folder))
        .and_then(|board| board.card(id).ok()?.asked())
        .map(|asked| asked.said.clone())
        .unwrap_or_default()
}

/// The asks still kept, and the address of every page opened under them.
pub(crate) fn kept(core: &Core) -> (BTreeSet<u64>, Vec<String>) {
    let state = core.boards().state.locked();
    let all = || state.boards.values().flat_map(asks);
    let ids = all().map(|(card, _)| card.id).collect();
    let opened = all()
        .flat_map(|(_, asked)| asked.pages.iter().map(|page| page.url.clone()))
        .collect();
    (ids, opened)
}

/// The window's desks moved from `before` to `after`. What opened on an agent's desk goes under
/// the ask the person has open there, else the ask the agent is answering; pages already kept
/// take their latest title and address.
pub(crate) fn desks_moved(
    core: &Core,
    before: &BTreeMap<String, AgentDesk>,
    after: &BTreeMap<String, AgentDesk>,
) {
    let opened: Vec<(String, u64, Vec<DeskPage>, Vec<String>)> = (after.iter())
        .filter_map(|(agent, desk)| {
            let held = before.get(agent);
            let pages: Vec<DeskPage> = (desk.pages.iter())
                .filter(|page| {
                    held.is_none_or(|held| !held.pages.iter().any(|it| it.id == page.id))
                })
                .cloned()
                .collect();
            let files: Vec<String> = (desk.files.iter())
                .filter(|file| held.is_none_or(|held| !held.files.contains(file)))
                .cloned()
                .collect();
            if pages.is_empty() && files.is_empty() {
                return None;
            }
            let ask = desk.ask.or_else(|| core.chats().get(agent)?.answering())?;
            Some((folder_of_agent(core, agent)?, ask, pages, files))
        })
        .collect();
    let shown: Vec<&DeskPage> = after.values().flat_map(|desk| &desk.pages).collect();
    let moved = |kept: &DeskPage| shown.iter().any(|page| page.id == kept.id && *page != kept);
    let retitled = (core.boards().state.locked().boards.values())
        .flat_map(asks)
        .any(|(_, asked)| asked.pages.iter().any(&moved));
    if opened.is_empty() && !retitled {
        return;
    }
    change(core, |board, at| {
        let folder = board.folder().to_owned();
        each_ask(board, at, |id, asked| {
            let mut changed = false;
            let here = (opened.iter()).filter(|(on, ask, ..)| *on == folder && *ask == id);
            for (_, _, pages, files) in here {
                for page in pages {
                    if !asked.pages.iter().any(|kept| kept.id == page.id) {
                        asked.pages.push(page.clone());
                        changed = true;
                    }
                }
                for file in files {
                    if !asked.files.contains(file) {
                        asked.files.push(file.clone());
                        changed = true;
                    }
                }
                let over = asked.pages.len().saturating_sub(MOST_OPENED);
                asked.pages.drain(..over);
                let over = asked.files.len().saturating_sub(MOST_OPENED);
                asked.files.drain(..over);
            }
            for kept in asked.pages.iter_mut() {
                if let Some(now) = shown
                    .iter()
                    .find(|page| page.id == kept.id && **page != kept)
                {
                    *kept = (*now).clone();
                    changed = true;
                }
            }
            changed
        })
    });
}

/// `from` started or messaged `to`: the asks of its turn go with it, and so do the asks of its
/// project named. A start's `task` names those that have no name yet.
pub(crate) fn handed(core: &Core, from: &str, to: &[String], named: &[u64], task: Option<&str>) {
    if to.is_empty() {
        return;
    }
    let holders: Vec<Holder> = to.iter().map(|agent| holder(core, agent)).collect();
    let home = folder_of_agent(core, from);
    change(core, |board, at| {
        let names_here = home.as_deref() == Some(board.folder());
        each_ask(board, at, |id, asked| {
            let ours = asked.to == from && asked.in_turn;
            if !ours && !(names_here && named.contains(&id)) {
                return false;
            }
            let mut changed = false;
            for holder in &holders {
                if !asked.owners.iter().any(|kept| kept.agent == holder.agent) {
                    asked.owners.push(holder.clone());
                    changed = true;
                }
            }
            changed |= asked.nobody_since.take().is_some();
            if let Some(task) = task.filter(|_| asked.name.is_none()) {
                asked.name = Some(clipped_line(task, TITLE_CHARS));
                changed = true;
            }
            changed
        })
    });
}

/// `agent`'s turn ended by itself. A chief's asks it handed to nobody were answered; another
/// agent keeps its own.
pub(crate) fn turn_ended(core: &Core, agent: &str) {
    let keeps = !chief::is_chief(core, agent);
    change(core, |board, at| {
        let mut answered = Vec::new();
        let changed = each_ask(board, at, |id, asked| {
            if asked.to != agent || !asked.in_turn {
                return false;
            }
            asked.in_turn = false;
            match (keeps, asked.owners.is_empty()) {
                (_, false) => {}
                (false, true) => answered.push(id),
                (true, true) => asked.owners.push(Holder {
                    agent: agent.to_owned(),
                    before: asked.to_before.clone(),
                }),
            }
            true
        });
        board.forget_answered(|card| answered.contains(&card.id)) || changed
    });
}

/// Where `agent`'s work is was read again: an ask it holds reaches the branch with its work, and
/// one it kept for itself was answered when its work did not move.
pub(crate) fn work_read(core: &Core, agent: &str) {
    let fix = super::fix_now(core, agent);
    let now = stood(core, agent);
    change(core, |board, at| {
        let mut answered = Vec::new();
        let changed = each_ask(board, at, |id, asked| {
            let Some(held) = asked.owners.iter().find(|holder| holder.agent == agent) else {
                return false;
            };
            if asked.in_turn {
                return false;
            }
            let moved_here = moved(now.as_ref(), held);
            let own = asked.to == agent && asked.owners.len() == 1;
            if own && !moved_here {
                answered.push(id);
                return false;
            }
            match fix.as_ref().filter(|_| moved_here) {
                Some(fix) => reached(asked, fix, at),
                None => false,
            }
        });
        board.forget_answered(|card| answered.contains(&card.id)) || changed
    });
}

/// Takes `fix` as how far `asked` has gone; whether that moved it.
fn reached(asked: &mut Asked, fix: &Fix, at: u64) -> bool {
    if fix.at < FixAt::Landed {
        return false;
    }
    let mut changed = asked.commit.as_deref() != Some(&fix.commit);
    asked.commit = Some(fix.commit.clone());
    asked.nobody_since = None;
    changed |= asked.landed_at.is_none();
    asked.landed_at.get_or_insert(at);
    if fix.at == FixAt::Running && asked.in_build_at.is_none() {
        asked.in_build_at = Some(at);
        changed = true;
    }
    changed
}

/// `agent` is closing: it lets go of its asks, and an ask nobody else holds whose work is not on
/// the branch has nobody, unless its work never moved, which answered it.
pub(crate) fn closing(core: &Core, agent: &str) {
    let fix = super::fix_now(core, agent);
    let now = stood(core, agent);
    change(core, |board, at| {
        let mut answered = Vec::new();
        let changed = each_ask(board, at, |id, asked| {
            let mid_turn = asked.to == agent && asked.in_turn;
            let held = asked.owners.iter().find(|holder| holder.agent == agent);
            let Some(moved) = held.map(|held| moved(now.as_ref(), held)) else {
                if mid_turn && asked.owners.is_empty() {
                    asked.in_turn = false;
                    asked.nobody_since = Some(at);
                    return true;
                }
                return false;
            };
            if let Some(fix) = fix.as_ref().filter(|_| moved) {
                reached(asked, fix, at);
            }
            asked.owners.retain(|holder| holder.agent != agent);
            asked.in_turn &= !mid_turn;
            if !asked.owners.is_empty() || asked.commit.is_some() {
                return true;
            }
            if !moved && !mid_turn {
                answered.push(id);
                return true;
            }
            asked.nobody_since = Some(at);
            true
        });
        board.forget_answered(|card| answered.contains(&card.id)) || changed
    });
}

/// The build the person runs, or the project's tags, moved: asks of the project at `repo` whose
/// commit is now in that build say so.
pub(super) fn build_moved(core: &Core, repo: &Path) {
    let Some(build) = running_build(core, repo) else {
        return;
    };
    let folder = repo.to_string_lossy().into_owned();
    let waiting: Vec<(u64, String)> = (core.boards().state.locked().of(&folder))
        .into_iter()
        .flat_map(asks)
        .filter(|(_, asked)| asked.in_build_at.is_none())
        .filter_map(|(card, asked)| Some((card.id, asked.commit.clone()?)))
        .collect();
    let built: Vec<u64> = (waiting.into_iter())
        .filter(|(_, commit)| {
            Git::quick(repo)
                .output(&["merge-base", "--is-ancestor", commit, &build])
                .is_ok_and(|output| output.status.success())
        })
        .map(|(id, _)| id)
        .collect();
    if built.is_empty() {
        return;
    }
    change_in(core, &folder, |board, at| {
        each_ask(board, at, |id, asked| {
            if !built.contains(&id) || asked.in_build_at.is_some() {
                return false;
            }
            asked.in_build_at = Some(at);
            true
        })
    });
}

/// The asks of the project at `folder` as the Now card lists them.
pub(super) fn of_project(core: &Core, folder: &str, at: u64) -> Vec<NowAsk> {
    let state = core.boards().state.locked();
    let Some(board) = state.of(folder) else {
        return Vec::new();
    };
    let mut shown: Vec<NowAsk> = asks(board)
        .filter(|(_, asked)| {
            asked
                .in_build_at
                .is_none_or(|since| at.saturating_sub(since) < SHOWN_IN_BUILD_MS)
        })
        .map(|(card, asked)| {
            let (stand, since) = match (asked.in_build_at, &asked.commit, asked.nobody_since) {
                (Some(since), _, _) => (AskStand::InYourBuild, since),
                (None, Some(_), _) => (AskStand::OnBranch, asked.landed_at.unwrap_or(card.at())),
                (None, None, Some(since)) => (AskStand::Nobody, since),
                (None, None, None) => {
                    let agents = match asked.owners.is_empty() {
                        true => vec![asked.to.clone()],
                        false => (asked.owners.iter())
                            .map(|holder| holder.agent.clone())
                            .collect(),
                    };
                    (AskStand::Working { agents }, card.at())
                }
            };
            NowAsk {
                id: card.id,
                title: card.title().to_owned(),
                name: asked.name.clone(),
                more: asked.said.len().saturating_sub(1) as u32,
                stand,
                since,
                images: asked.images.clone(),
                to: asked.to.clone(),
                at: card.at(),
                pages: asked.pages.clone(),
                files: asked.files.clone(),
            }
        })
        .collect();
    shown.sort_by_key(|ask| (rank(&ask.stand), ask.since));
    shown
}

fn rank(stand: &AskStand) -> u8 {
    match stand {
        AskStand::Nobody => 0,
        AskStand::Working { .. } => 1,
        AskStand::OnBranch => 2,
        AskStand::InYourBuild => 3,
        AskStand::Unknown => 4,
    }
}

/// An ask nobody has whose chief was not told of it yet.
pub(crate) struct Untold {
    pub id: u64,
    /// The project's folder.
    pub folder: String,
    pub title: String,
    /// Since when nobody has it.
    pub since: u64,
}

pub(crate) fn with_nobody_untold(core: &Core) -> Vec<Untold> {
    let state = core.boards().state.locked();
    (state.boards.values())
        .flat_map(|board| asks(board).map(move |(card, asked)| (board, card, asked)))
        .filter(|(_, _, asked)| asked.commit.is_none() && asked.in_build_at.is_none())
        .filter_map(|(board, card, asked)| {
            let since = asked.nobody_since?;
            (asked.told_since != Some(since)).then(|| Untold {
                id: card.id,
                folder: board.folder().to_owned(),
                title: card.title().to_owned(),
                since,
            })
        })
        .collect()
}

/// The chiefs were told the asks `told`, each by folder and number, have nobody.
pub(crate) fn told(core: &Core, told: &[(String, u64)]) {
    change(core, |board, at| {
        let folder = board.folder().to_owned();
        each_ask(board, at, |id, asked| {
            if !told.iter().any(|(on, told)| *on == folder && *told == id) {
                return false;
            }
            let changed = asked.told_since != asked.nobody_since;
            asked.told_since = asked.nobody_since;
            changed
        })
    });
}

/// The full id of the commit `named` in the project at `folder`, or in a repository the folder holds.
fn commit_in(folder: &Path, named: &str) -> Option<String> {
    let held = std::fs::read_dir(folder).into_iter().flatten().flatten();
    let held = held
        .map(|entry| entry.path())
        .filter(|child| child.join(".git").exists());
    std::iter::once(folder.to_path_buf())
        .chain(held)
        .find_map(|repo| {
            Git::quick(&repo)
                .trimmed(&[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    "--end-of-options",
                    &format!("{named}^{{commit}}"),
                ])
                .ok()
                .filter(|commit| !commit.is_empty())
        })
}

/// `task_update` with `asked`: the caller closes one of its project's asks that was handled: in the commit it
/// names, which then stands for the ask's work, or with nothing to change, which answered it.
pub(crate) fn done(core: &Core, request: &HarnessRequest) -> Result<Value, String> {
    let agent = (request.agent_id.as_deref()).ok_or("task_update needs the calling agent's id")?;
    let id = (request.params.get("asked"))
        .and_then(Value::as_u64)
        .ok_or("task_update needs the number of what the person asked for")?;
    let project = (core.workspaces().agent(agent))
        .ok_or("task_update answers only an agent Sikemux started")?
        .project;
    let folder = folder_of(core, &project).ok_or("this project has no folder")?;
    let ours = (core.boards().state.locked().of(&folder))
        .and_then(|board| board.card(id).ok())
        .is_some_and(|card| card.asked().is_some());
    if !ours {
        return Err(format!("task #{id} is not an open task of this project"));
    }
    let named = (request.params.get("commit").and_then(Value::as_str))
        .map(str::trim)
        .filter(|it| !it.is_empty());
    let Some(named) = named else {
        change_in(core, &folder, |board, _| {
            board.forget_answered(|card| card.id == id)
        });
        return Ok(json!({ "closed": id, "answered": "no change" }));
    };
    let repo = PathBuf::from(&folder);
    let commit = commit_in(&repo, named)
        .ok_or_else(|| format!("{named} is not a commit of this project"))?;
    change_in(core, &folder, |board, at| {
        let Ok(card) = board.card_mut(id) else {
            return false;
        };
        card.change_asked(at, |asked| {
            asked.commit = Some(commit.clone());
            asked.landed_at.get_or_insert(at);
            asked.nobody_since = None;
            asked.in_turn = false;
            asked.owners.clear();
        })
        .is_some()
    });
    build_moved(core, &repo);
    Ok(json!({ "closed": id, "commit": commit }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_commit_of_a_repository_inside_the_project_folder_is_found() {
        let folder = tempfile::tempdir().expect("tempdir");
        let web = folder.path().join("web");
        std::fs::create_dir(&web).expect("repository folder");
        Git::quick(&web).trimmed(&["init", "-q"]).expect("init");
        Git::quick(&web)
            .trimmed(&[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "c",
            ])
            .expect("commit");
        let head = Git::quick(&web)
            .trimmed(&["rev-parse", "HEAD"])
            .expect("head");

        assert_eq!(
            commit_in(folder.path(), head.get(..9).expect("short id")),
            Some(head)
        );
        assert_eq!(commit_in(folder.path(), "0000000"), None);
    }
}
