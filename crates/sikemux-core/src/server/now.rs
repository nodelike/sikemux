//! Each project's Now strip: its open agents in the person's four groups, each with one fact and
//! since when, read off the turn states the device view works out anyway, and the agents closed
//! lately, which closing writes in. Agents read the same strip as one compact text.

use crate::server::parts::Parts;
use crate::server::workspace_state::WorkspaceParts;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::chat::FailureKind;
use sikemux_view::now::{
    ClosedWhy, FixAt, NowClosed, NowFact, NowGroup, NowRelease, NowRow, ProjectNow,
};
use sikemux_view::turn::{ChecksStand, TurnPhase, TurnState, TurnWaitKind, WorkAt};

use super::changed::{ChangedCore, What};
use super::kept::Kept;
use super::Core;

const FILE: &str = "now.json";
/// Closed agents a project's strip keeps, newest kept.
const MOST_CLOSED: usize = 30;

pub(crate) struct Now {
    kept: Kept,
    state: Mutex<Saved>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Saved {
    /// By project id, newest first.
    closed: HashMap<String, Vec<NowClosed>>,
}

impl Now {
    pub(crate) fn new(data_dir: Option<&Path>) -> Self {
        let kept = Kept::new(data_dir, FILE);
        let state = Mutex::new(kept.load());
        Self { kept, state }
    }

    fn change(&self, change: impl FnOnce(&mut Saved)) {
        let mut state = self.state.locked();
        change(&mut state);
        self.kept.save_soon(&*state);
    }
}

impl super::part::Part for Now {
    fn flush(&self) {
        self.kept.flush();
    }
}

/// An agent of `project` closed: its strip logs it, newest first, until it is reopened.
#[expect(dead_code, reason = "auto-close calls it")]
pub(crate) fn closed(core: &Core, project: &str, entry: NowClosed) {
    core.now().change(|saved| {
        let log = saved.closed.entry(project.to_owned()).or_default();
        log.retain(|kept| kept.agent != entry.agent);
        log.insert(0, entry);
        log.truncate(MOST_CLOSED);
    });
    core.changed(What::View);
}

/// The closed agent `agent` was opened again, so the log lets it go.
pub(crate) fn reopened(core: &Core, agent: &str) {
    let mut gone = false;
    core.now().change(|saved| {
        for log in saved.closed.values_mut() {
            let before = log.len();
            log.retain(|kept| kept.agent != agent);
            gone |= log.len() != before;
        }
    });
    if gone {
        core.changed(What::View);
    }
}

/// An open agent as the strip reads it.
pub(crate) struct Open<'a> {
    pub agent: &'a str,
    /// The workspace project, by id.
    pub project: &'a str,
    pub turn: Option<&'a TurnState>,
}

/// Every project's strip, from its open agents and how their turns stand. Chiefs have no row.
pub(crate) fn projects<'a>(
    core: &Core,
    agents: impl IntoIterator<Item = Open<'a>>,
) -> Vec<ProjectNow> {
    let workspace = core.workspaces();
    let chiefs = workspace.chiefs();
    let leads = workspace.agent_leads();
    let agents: Vec<Open> = agents.into_iter().collect();
    let open = |id: &str| agents.iter().any(|agent| agent.agent == id);
    let mut strips: HashMap<String, Vec<NowRow>> = HashMap::new();
    for agent in &agents {
        if chiefs.contains(agent.agent) {
            continue;
        }
        let Some((group, fact, since)) = agent.turn.and_then(row_of) else {
            continue;
        };
        let lead = (leads.get(agent.agent))
            .filter(|lead| !chiefs.contains(*lead) && open(lead))
            .cloned();
        strips
            .entry(agent.project.to_owned())
            .or_default()
            .push(NowRow {
                agent: agent.agent.to_owned(),
                group,
                lead,
                fact,
                since,
            });
    }
    let closed = core.now().state.locked().closed.clone();
    let mut projects: Vec<ProjectNow> = (workspace.projects().into_iter())
        .filter_map(|project| {
            let mut rows = strips.remove(&project.id).unwrap_or_default();
            rows.sort_by_key(|row| (row.group, row.since));
            let closed = closed.get(&project.id).cloned().unwrap_or_default();
            (!rows.is_empty() || !closed.is_empty()).then_some(ProjectNow {
                project: project.id,
                rows,
                release: None,
                closed,
            })
        })
        .collect();
    projects.sort_by(|a, b| a.project.cmp(&b.project));
    projects
}

/// The strip of the project at `path`, worked out now, as agents read it.
pub(crate) fn of_path(core: &Core, path: &str) -> Option<ProjectNow> {
    let project = (core.workspaces().projects().into_iter())
        .find(|project| project.path.to_string_lossy() == path)?;
    let turns = super::turn_state::all(core);
    let projects = core.workspaces().agent_projects();
    let open = (projects.iter())
        .filter(|(_, of)| **of == project.id)
        .map(|(agent, of)| Open {
            agent,
            project: of,
            turn: turns.get(agent),
        });
    let strip = (self::projects(core, open).into_iter()).find(|now| now.project == project.id);
    Some(strip.unwrap_or(ProjectNow {
        project: project.id,
        rows: Vec::new(),
        release: None,
        closed: Vec::new(),
    }))
}

/// Which group a turn puts its agent in, with its one fact and since when. None for an agent
/// with nothing to show: before its first turn, or done with nothing changed and seen.
fn row_of(turn: &TurnState) -> Option<(NowGroup, NowFact, u64)> {
    let since = turn.since;
    Some(match turn.state {
        TurnPhase::NeedsYou => {
            let count = turn.call.as_ref().map_or(1, |call| call.count.max(1));
            (NowGroup::WaitingOnYou, NowFact::Asks { count }, since)
        }
        TurnPhase::Failed => {
            let why = (turn.failed.as_ref()).map_or(FailureKind::Other, |failed| failed.why);
            let group = match why {
                FailureKind::SignIn => NowGroup::WaitingOnYou,
                _ => NowGroup::NotSure,
            };
            (group, NowFact::Failed { why }, since)
        }
        TurnPhase::Stopped => (NowGroup::NotSure, NowFact::Stopped, since),
        TurnPhase::Working => (
            NowGroup::Moving,
            NowFact::Working {
                on: turn.on.clone(),
            },
            since,
        ),
        TurnPhase::Waiting => {
            let wait = turn.wait.as_ref()?;
            let fact = NowFact::Waiting {
                on: wait.kind,
                what: wait.what.clone(),
                agent: wait.agent.clone(),
                place: wait.place,
                expected_ms: wait.expected_ms,
            };
            (NowGroup::Moving, fact, since)
        }
        TurnPhase::Done => {
            let changes = (turn.done.as_ref()).map_or(WorkAt::None, |done| done.changes);
            let Some(at) = fix_at(changes) else {
                return (!turn.seen).then_some((NowGroup::ReadyToLook, NowFact::Answered, since));
            };
            let checks = (turn.done.as_ref()).map_or(ChecksStand::NotChecked, |done| done.checks);
            let group = match at {
                FixAt::Uncommitted | FixAt::Committed => NowGroup::NotSure,
                _ => NowGroup::ReadyToLook,
            };
            let fact = NowFact::Work {
                at,
                checks,
                release: None,
            };
            (group, fact, since)
        }
        TurnPhase::Unknown => (NowGroup::NotSure, NowFact::Unknown, since),
    })
}

fn fix_at(changes: WorkAt) -> Option<FixAt> {
    Some(match changes {
        WorkAt::Uncommitted => FixAt::Uncommitted,
        WorkAt::Committed => FixAt::Committed,
        WorkAt::Landed => FixAt::Landed,
        WorkAt::Staged => FixAt::Staged,
        WorkAt::Shipped => FixAt::Shipped,
        WorkAt::None | WorkAt::Unknown => return None,
    })
}

/// The strip as compact text for agents, one line per group, each agent as `@name`.
pub(crate) fn text(now: &ProjectNow, name: impl Fn(&str) -> String, at: u64) -> String {
    let named = |agent: &str| format!("@{}", name(agent));
    let mut lines = Vec::new();
    let groups = [
        (NowGroup::WaitingOnYou, "Waiting on you"),
        (NowGroup::ReadyToLook, "Ready to look at"),
        (NowGroup::Moving, "Moving"),
        (NowGroup::NotSure, "Not sure"),
    ];
    for (group, title) in groups {
        let rows: Vec<String> = (now.rows.iter())
            .filter(|row| row.group == group)
            .map(|row| {
                let under = (row.lead.as_deref())
                    .map(|lead| format!(" (for {})", named(lead)))
                    .unwrap_or_default();
                let fact = fact_words(&row.fact, &named);
                format!(
                    "{}{under} {fact}, {}",
                    named(&row.agent),
                    ago(row.since, at)
                )
            })
            .collect();
        if !rows.is_empty() {
            lines.push(format!("{title}: {}", rows.join("; ")));
        }
    }
    if let Some(release) = &now.release {
        lines.push(release_words(release, at));
    }
    if !now.closed.is_empty() {
        let closed: Vec<String> = (now.closed.iter())
            .map(|closed| {
                let why = match closed.why {
                    ClosedWhy::Landed => "landed",
                    ClosedWhy::NothingChanged => "changed nothing",
                    ClosedWhy::ByAgent => "by an agent",
                    ClosedWhy::ByPerson => "by the person",
                    ClosedWhy::Unknown => "closed",
                };
                format!("@{} {why}, {}", closed.name, ago(closed.at, at))
            })
            .collect();
        lines.push(format!("Closed: {}", closed.join("; ")));
    }
    match lines.is_empty() {
        true => "Nothing open.".to_owned(),
        false => lines.join("\n"),
    }
}

fn fact_words(fact: &NowFact, named: &impl Fn(&str) -> String) -> String {
    match fact {
        NowFact::Asks { count: 1 } => "asks you 1 thing".to_owned(),
        NowFact::Asks { count } => format!("asks you {count} things"),
        NowFact::Failed { why } => match why {
            FailureKind::Limit => "failed on the usage limit",
            FailureKind::SignIn => "failed, signed out",
            FailureKind::Network => "failed on the network",
            FailureKind::Other | FailureKind::Unknown => "failed",
        }
        .to_owned(),
        NowFact::Stopped => "turn ended, nothing set to wake it".to_owned(),
        NowFact::Working { on: Some(on) } => format!("working on {on}"),
        NowFact::Working { on: None } => "working".to_owned(),
        NowFact::Waiting {
            on,
            what,
            agent,
            place,
            ..
        } => {
            let on_what = match on {
                TurnWaitKind::Agent => {
                    let who = agent
                        .as_deref()
                        .map(named)
                        .unwrap_or_else(|| format!("@{what}"));
                    return format!("waiting on {who}");
                }
                TurnWaitKind::Line => "in line",
                TurnWaitKind::InUse => "waiting on something in use",
                TurnWaitKind::Ci => "waiting on CI",
                TurnWaitKind::Deploy => "waiting on a deploy",
                TurnWaitKind::Timer => "waiting on a timer",
                TurnWaitKind::Process => "waiting on a process",
                TurnWaitKind::Check | TurnWaitKind::Unknown => "waiting on the project's check",
            };
            let place = place
                .map(|place| format!(", place {place}"))
                .unwrap_or_default();
            match what.is_empty() {
                true => format!("{on_what}{place}"),
                false => format!("{on_what} for {what}{place}"),
            }
        }
        NowFact::Work {
            at,
            checks,
            release,
        } => {
            let at = match (at, release) {
                (FixAt::Released, Some(tag)) => format!("in {tag}"),
                (FixAt::Running, _) => "in the app you run".to_owned(),
                (FixAt::Uncommitted, _) => "written, uncommitted".to_owned(),
                (FixAt::Committed, _) => "committed, not landed".to_owned(),
                (FixAt::Landed | FixAt::Released, _) => "landed".to_owned(),
                (FixAt::Staged, _) => "on staging".to_owned(),
                (FixAt::Shipped, _) => "shipped".to_owned(),
                (FixAt::Unknown, _) => "done".to_owned(),
            };
            match checks {
                ChecksStand::Failed => format!("{at}, checks failed"),
                _ => at,
            }
        }
        NowFact::Answered => "answered, changed nothing".to_owned(),
        NowFact::Unknown => "state unknown".to_owned(),
    }
}

fn release_words(release: &NowRelease, at: u64) -> String {
    let mut words = vec![match (&release.tag, release.tagged_at) {
        (Some(tag), Some(made)) => format!("Release: {tag}, {}", ago(made, at)),
        (Some(tag), None) => format!("Release: {tag}"),
        (None, _) => "Release: none yet".to_owned(),
    }];
    words.push(format!("{} on {} since", release.since_tag, release.branch));
    if let Some(ci) = release.ci {
        words.push(
            match ci {
                sikemux_view::now::CiStand::Passed => "CI green",
                sikemux_view::now::CiStand::Running => "CI running",
                sikemux_view::now::CiStand::Failed => "CI red",
                sikemux_view::now::CiStand::Unknown => "CI unknown",
            }
            .to_owned(),
        );
    }
    if release.releasing {
        words.push("a release is running".to_owned());
    }
    if let Some(running) = &release.running {
        words.push(format!("you run {running}"));
    }
    words.join(", ")
}

fn ago(since: u64, at: u64) -> String {
    let minutes = at.saturating_sub(since) / 60_000;
    match minutes {
        0 => "just now".to_owned(),
        1..60 => format!("{minutes} min"),
        60..1440 => format!("{} h", minutes / 60),
        _ => format!("{} d", minutes / 1440),
    }
}

/// The strip of the project at `path` as agents read it, or none outside every project.
pub(crate) fn text_of_path(core: &Arc<Core>, path: &str) -> Option<String> {
    let now = of_path(core, path)?;
    let names = core.names();
    Some(text(
        &now,
        |agent| names.name(agent).unwrap_or_else(|| agent.to_owned()),
        epoch_ms(),
    ))
}
