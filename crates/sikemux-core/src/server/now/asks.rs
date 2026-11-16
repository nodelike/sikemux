//! The person's asks: one for each message they send an agent, kept from then until its change is
//! in the app they run. The agents the receiving agent starts or messages in that turn take it
//! over. A chief's turn that hands it to nobody answered it, as did another agent's turn that left
//! its work where it was. An ask outlives the agents on it: once every one has closed before its
//! work reached the project's branch, nobody has it.

use super::*;
use crate::server::chat_state::ChatParts;
use crate::server::chief;
use crate::text::clipped_line;
use sikemux_view::goals::{AgentDesk, DeskPage};
use std::collections::BTreeMap;

/// Kept per core; the oldest go first.
const MOST: usize = 300;
const TITLE_CHARS: usize = 160;
/// How long an ask stays on the card once its change is in the app the person runs.
const SHOWN_IN_BUILD_MS: u64 = 24 * 60 * 60 * 1000;
/// The same words to the same agent within this long are one ask, sent again.
const SAME_ASK_MS: u64 = 10 * 60 * 1000;
/// The most pages, and the most files, an ask keeps of what was opened while it was answered.
const MOST_OPENED: usize = 24;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Ask {
    id: u64,
    /// The workspace project, by id.
    project: String,
    title: String,
    at: u64,
    /// The agent the person sent it to.
    to: String,
    /// Where that agent's work stood as the person sent it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    to_before: Option<Stood>,
    /// Still in the turn of `to` it came in, whose starts and messages hand it on.
    #[serde(default)]
    in_turn: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    owners: Vec<Owner>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    nobody_since: Option<u64>,
    /// The newest commit of its owners' work on the project's branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    landed_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    in_build_at: Option<u64>,
    /// What opened on the desk of `to` while it answered this ask or the person had it open.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pages: Vec<DeskPage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    files: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Owner {
    agent: String,
    /// Where its work stood as it took the ask, so only work done since counts for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    before: Option<Stood>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Stood {
    changes: WorkAt,
    head: Option<String>,
}

fn stood(core: &Core, agent: &str) -> Option<Stood> {
    let work = core.inbox().ending(agent)?.work?;
    Some(Stood {
        changes: work.changes,
        head: work.head,
    })
}

fn owner(core: &Core, agent: &str) -> Owner {
    Owner {
        agent: agent.to_owned(),
        before: stood(core, agent),
    }
}

/// Whether `owner`'s work moved since it took the ask.
fn moved(core: &Core, owner: &Owner) -> bool {
    stood(core, &owner.agent)
        .is_some_and(|now| now.changes != WorkAt::None && owner.before.as_ref() != Some(&now))
}

fn change(core: &Core, change: impl FnOnce(&mut Vec<Ask>, &mut u64) -> bool) {
    let mut changed = false;
    core.now().change(|saved| {
        changed = change(&mut saved.asks, &mut saved.next_ask);
        if changed {
            let at = epoch_ms();
            saved.asks.retain(|ask| {
                ask.in_build_at
                    .is_none_or(|since| at.saturating_sub(since) < SHOWN_IN_BUILD_MS)
            });
            let over = saved.asks.len().saturating_sub(MOST);
            saved.asks.drain(..over);
        }
    });
    if changed {
        core.changed(What::View);
    }
}

/// The person sent `text` to `agent`, as a reply on the ask `filed`
/// when it names one. The ask it is, by number.
pub(crate) fn asked(core: &Core, agent: &str, text: &str, filed: Option<u64>) -> Option<u64> {
    if filed.is_some() {
        return filed;
    }
    let title = clipped_line(text, TITLE_CHARS);
    if title.is_empty() {
        return None;
    }
    let project = core.workspaces().agent(agent)?.project;
    let at = epoch_ms();
    let before = stood(core, agent);
    let mut id = None;
    change(core, |asks, next| {
        let again = asks.iter().find(|ask| {
            ask.to == agent && ask.title == title && at.saturating_sub(ask.at) < SAME_ASK_MS
        });
        if let Some(again) = again {
            id = Some(again.id);
            return false;
        }
        *next += 1;
        id = Some(*next);
        asks.push(Ask {
            id: *next,
            project,
            title,
            at,
            to: agent.to_owned(),
            to_before: before,
            in_turn: true,
            owners: Vec::new(),
            nobody_since: None,
            commit: None,
            landed_at: None,
            in_build_at: None,
            pages: Vec::new(),
            files: Vec::new(),
        });
        true
    });
    id
}

/// The asks still kept, and the address of every page opened under them.
pub(crate) fn kept(core: &Core) -> (std::collections::BTreeSet<u64>, Vec<String>) {
    let state = core.now().state.locked();
    let ids = state.asks.iter().map(|ask| ask.id).collect();
    let opened = (state.asks.iter())
        .flat_map(|ask| ask.pages.iter().map(|page| page.url.clone()))
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
    let opened: Vec<(u64, Vec<DeskPage>, Vec<String>)> = (after.iter())
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
            Some((ask, pages, files))
        })
        .collect();
    let shown: Vec<&DeskPage> = after.values().flat_map(|desk| &desk.pages).collect();
    let moved = |kept: &DeskPage| shown.iter().any(|page| page.id == kept.id && *page != kept);
    let retitled = (core.now().state.locked().asks.iter()).any(|ask| ask.pages.iter().any(&moved));
    if opened.is_empty() && !retitled {
        return;
    }
    change(core, |asks, _| {
        let mut changed = false;
        for ask in asks.iter_mut() {
            for (_, pages, files) in opened.iter().filter(|(id, ..)| *id == ask.id) {
                for page in pages {
                    if !ask.pages.iter().any(|kept| kept.id == page.id) {
                        ask.pages.push(page.clone());
                        changed = true;
                    }
                }
                for file in files {
                    if !ask.files.contains(file) {
                        ask.files.push(file.clone());
                        changed = true;
                    }
                }
                let over = ask.pages.len().saturating_sub(MOST_OPENED);
                ask.pages.drain(..over);
                let over = ask.files.len().saturating_sub(MOST_OPENED);
                ask.files.drain(..over);
            }
            for kept in ask.pages.iter_mut() {
                if let Some(now) = shown
                    .iter()
                    .find(|page| page.id == kept.id && **page != kept)
                {
                    *kept = (*now).clone();
                    changed = true;
                }
            }
        }
        changed
    });
}

/// `from` started or messaged `to`: the asks of its turn go with it, and so do the asks named.
pub(crate) fn handed(core: &Core, from: &str, to: &[String], named: &[u64]) {
    if to.is_empty() {
        return;
    }
    let owners: Vec<Owner> = to.iter().map(|agent| owner(core, agent)).collect();
    change(core, |asks, _| {
        let mut changed = false;
        for ask in asks.iter_mut() {
            let ours = ask.to == from && ask.in_turn;
            if !ours && !named.contains(&ask.id) {
                continue;
            }
            for owner in &owners {
                if !ask.owners.iter().any(|kept| kept.agent == owner.agent) {
                    ask.owners.push(owner.clone());
                    changed = true;
                }
            }
            changed |= ask.nobody_since.take().is_some();
        }
        changed
    });
}

/// `agent`'s turn ended by itself. A chief's asks it handed to nobody were answered; another
/// agent keeps its own.
pub(crate) fn turn_ended(core: &Core, agent: &str) {
    let keeps = !chief::is_chief(core, agent);
    change(core, |asks, _| {
        let before = asks.len();
        let mut changed = false;
        asks.retain_mut(|ask| {
            if ask.to != agent || !ask.in_turn {
                return true;
            }
            ask.in_turn = false;
            changed = true;
            match (keeps, ask.owners.is_empty()) {
                (_, false) => true,
                (false, true) => false,
                (true, true) => {
                    ask.owners.push(Owner {
                        agent: agent.to_owned(),
                        before: ask.to_before.clone(),
                    });
                    true
                }
            }
        });
        changed || asks.len() != before
    });
}

/// Where `agent`'s work is was read again: an ask it holds reaches the branch with its work, and
/// one it kept for itself was answered when its work did not move.
pub(crate) fn work_read(core: &Core, agent: &str) {
    let fix = super::fix_now(core, agent);
    let moved_here = |owner: &Owner| moved(core, owner);
    let at = epoch_ms();
    change(core, |asks, _| {
        let mut changed = false;
        asks.retain_mut(|ask| {
            let Some(held) = ask.owners.iter().find(|owner| owner.agent == agent) else {
                return true;
            };
            if ask.in_turn {
                return true;
            }
            let own = ask.to == agent && ask.owners.len() == 1;
            if own && !moved_here(held) {
                changed = true;
                return false;
            }
            if let Some(fix) = fix.as_ref().filter(|_| moved_here(held)) {
                changed |= reached(ask, fix, at);
            }
            true
        });
        changed
    });
}

/// Takes `fix` as how far `ask` has gone; whether that moved it.
fn reached(ask: &mut Ask, fix: &Fix, at: u64) -> bool {
    if fix.at < FixAt::Landed {
        return false;
    }
    let mut changed = ask.commit.as_deref() != Some(&fix.commit);
    ask.commit = Some(fix.commit.clone());
    ask.nobody_since = None;
    changed |= ask.landed_at.is_none();
    ask.landed_at.get_or_insert(at);
    if fix.at == FixAt::Running && ask.in_build_at.is_none() {
        ask.in_build_at = Some(at);
        changed = true;
    }
    changed
}

/// `agent` is closing: it lets go of its asks, and an ask nobody else holds whose work is not on
/// the branch has nobody, unless its work never moved, which answered it.
pub(crate) fn closing(core: &Core, agent: &str) {
    let fix = super::fix_now(core, agent);
    let at = epoch_ms();
    let held = |ask: &Ask| {
        ask.owners
            .iter()
            .find(|owner| owner.agent == agent)
            .cloned()
    };
    let moves: Vec<(u64, bool)> = (core.now().state.locked().asks.iter())
        .filter_map(|ask| Some((ask.id, moved(core, &held(ask)?))))
        .collect();
    change(core, |asks, _| {
        let before = asks.len();
        let mut changed = false;
        asks.retain_mut(|ask| {
            let mid_turn = ask.to == agent && ask.in_turn;
            let Some(&(_, moved)) = moves.iter().find(|(id, _)| *id == ask.id) else {
                if mid_turn && ask.owners.is_empty() {
                    ask.in_turn = false;
                    ask.nobody_since = Some(at);
                    changed = true;
                }
                return true;
            };
            if let Some(fix) = fix.as_ref().filter(|_| moved) {
                reached(ask, fix, at);
            }
            ask.owners.retain(|owner| owner.agent != agent);
            ask.in_turn &= !mid_turn;
            changed = true;
            if !ask.owners.is_empty() || ask.commit.is_some() {
                return true;
            }
            if !moved && !mid_turn {
                return false;
            }
            ask.nobody_since = Some(at);
            true
        });
        changed || asks.len() != before
    });
}

/// The build the person runs, or the project's tags, moved: asks of the project at `repo` whose
/// commit is now in that build say so.
pub(super) fn build_moved(core: &Core, project: &str, repo: &Path) {
    let Some(build) = running_build(core, repo) else {
        return;
    };
    let waiting: Vec<(u64, String)> = (core.now().state.locked().asks.iter())
        .filter(|ask| ask.project == project && ask.in_build_at.is_none())
        .filter_map(|ask| Some((ask.id, ask.commit.clone()?)))
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
    let at = epoch_ms();
    change(core, |asks, _| {
        for ask in asks.iter_mut().filter(|ask| built.contains(&ask.id)) {
            ask.in_build_at.get_or_insert(at);
        }
        true
    });
}

/// The project's asks as the card lists them.
pub(super) fn of_project(asks: &[Ask], project: &str, at: u64) -> Vec<NowAsk> {
    let mut shown: Vec<NowAsk> = (asks.iter())
        .filter(|ask| ask.project == project)
        .filter(|ask| {
            ask.in_build_at
                .is_none_or(|since| at.saturating_sub(since) < SHOWN_IN_BUILD_MS)
        })
        .map(|ask| {
            let (stand, since) = match (ask.in_build_at, &ask.commit, ask.nobody_since) {
                (Some(since), _, _) => (AskStand::InYourBuild, since),
                (None, Some(_), _) => (AskStand::OnBranch, ask.landed_at.unwrap_or(ask.at)),
                (None, None, Some(since)) => (AskStand::Nobody, since),
                (None, None, None) => {
                    let agents = match ask.owners.is_empty() {
                        true => vec![ask.to.clone()],
                        false => ask.owners.iter().map(|owner| owner.agent.clone()).collect(),
                    };
                    (AskStand::Working { agents }, ask.at)
                }
            };
            NowAsk {
                id: ask.id,
                title: ask.title.clone(),
                stand,
                since,
                to: ask.to.clone(),
                at: ask.at,
                pages: ask.pages.clone(),
                files: ask.files.clone(),
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

/// The asks nobody has, each with its id, project, title and since when nobody has it.
pub(crate) fn with_nobody(core: &Core) -> Vec<(u64, String, String, u64)> {
    (core.now().state.locked().asks.iter())
        .filter(|ask| ask.commit.is_none() && ask.in_build_at.is_none())
        .filter_map(|ask| {
            let since = ask.nobody_since?;
            Some((ask.id, ask.project.clone(), ask.title.clone(), since))
        })
        .collect()
}
