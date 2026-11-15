//! Each project's Now strip: its open agents in the person's four groups, each with one fact and
//! since when, read off the turn states the device view works out anyway, and the agents closed
//! lately, which closing writes in, and the person's asks. Agents read the same strip as one
//! compact text.

pub(crate) mod asks;

use crate::git::Git;
use crate::server::parts::Parts;
use crate::server::serving::ServeParts;
use crate::server::workspace_state::WorkspaceParts;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::chat::FailureKind;
use sikemux_view::now::{
    AskStand, CiStand, ClosedWhy, FixAt, NowAsk, NowClosed, NowFact, NowGroup, NowRelease, NowRow,
    ProjectNow,
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
    /// By agent, for agents whose work landed; read again on events, never per build.
    fixes: Mutex<HashMap<String, Fix>>,
    /// By project path.
    reads: Mutex<HashMap<String, Read>>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Saved {
    /// By project id, newest first.
    closed: HashMap<String, Vec<NowClosed>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    asks: Vec<asks::Ask>,
    #[serde(default)]
    next_ask: u64,
}

impl Now {
    pub(crate) fn new(data_dir: Option<&Path>) -> Self {
        let kept = Kept::new(data_dir, FILE);
        let state = Mutex::new(kept.load());
        Self {
            kept,
            state,
            fixes: Mutex::default(),
            reads: Mutex::default(),
        }
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

    fn start(&self, core: &Arc<Core>) -> Vec<tokio::task::JoinHandle<()>> {
        for project in core.workspaces().projects() {
            refs_moved(core, &project.path.to_string_lossy());
        }
        Vec::new()
    }
}

/// An agent of `project` closed: its strip logs it, newest first, until it is reopened.
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
    let (closed, asks) = {
        let state = core.now().state.locked();
        (state.closed.clone(), state.asks.clone())
    };
    let at = epoch_ms();
    {
        let fixes = core.now().fixes.locked();
        for row in strips.values_mut().flatten() {
            let (NowFact::Work { at, release, .. }, Some(fix)) =
                (&mut row.fact, fixes.get(&row.agent))
            else {
                continue;
            };
            if *at == FixAt::Landed {
                (*at, *release) = (fix.at, fix.release.clone());
            }
        }
    }
    let reads = core.now().reads.locked();
    let mut projects: Vec<ProjectNow> = (workspace.projects().into_iter())
        .filter_map(|project| {
            let mut rows = strips.remove(&project.id).unwrap_or_default();
            rows.sort_by_key(|row| (row.group, row.since));
            let closed = closed.get(&project.id).cloned().unwrap_or_default();
            let release =
                (reads.get(&*project.path.to_string_lossy())).and_then(|read| read.release.clone());
            let asks = asks::of_project(&asks, &project.id, at);
            let shows =
                !rows.is_empty() || !closed.is_empty() || !asks.is_empty() || release.is_some();
            shows.then_some(ProjectNow {
                project: project.id,
                rows,
                release,
                closed,
                asks,
            })
        })
        .collect();
    drop(reads);
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
    let release = (core.now().reads.locked().get(path)).and_then(|read| read.release.clone());
    Some(strip.unwrap_or(ProjectNow {
        project: project.id,
        rows: Vec::new(),
        release,
        closed: Vec::new(),
        asks: Vec::new(),
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
    lines.extend(asks_words(&now.asks, at));
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
                TurnWaitKind::Land => "waiting to land",
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

fn asks_words(asks: &[NowAsk], at: u64) -> Vec<String> {
    let nobody: Vec<String> = (asks.iter())
        .filter(|ask| ask.stand == AskStand::Nobody)
        .map(|ask| format!("#{} \"{}\", {}", ask.id, ask.title, ago(ask.since, at)))
        .collect();
    let count = |kind: fn(&AskStand) -> bool| asks.iter().filter(|ask| kind(&ask.stand)).count();
    let working = count(|stand| matches!(stand, AskStand::Working { .. }));
    let on_branch = count(|stand| *stand == AskStand::OnBranch);
    let mut lines = Vec::new();
    if !nobody.is_empty() {
        lines.push(format!(
            "The person's asks nobody is on (give one to an agent with agent_spawn's or agent_send's ask): {}",
            nobody.join("; ")
        ));
    }
    if working + on_branch > 0 {
        lines.push(format!(
            "The person's asks: {working} with agents, {on_branch} landed and not in the build they run"
        ));
    }
    lines
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

/// How far one agent's landed work has gone past the branch, as last read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fix {
    /// The commit that has its work: its newest own landed commit, else its checkout's head.
    commit: String,
    at: FixAt,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    release: Option<String>,
}

/// What a project's branch reads, kept as last read on a move of its refs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Read {
    release: Option<NowRelease>,
    /// The branch's tip when its CI was last asked, so a code host is asked only once per tip.
    ci_asked: Option<String>,
}

/// How long a CI run or a release run that was going waits before it is asked about again.
const CI_AGAIN: Duration = Duration::from_secs(60);

/// The branch of the project at `path` moved, or a tag came: its release row is read again off
/// the caller's thread, and when its newest tag or the app's build moved, where every one of
/// its agents' fixes is.
pub(crate) fn refs_moved(core: &Arc<Core>, path: &str) {
    let (core, path) = (core.clone(), path.to_owned());
    tokio::spawn(async move {
        let read = {
            let (core, path) = (core.clone(), path.clone());
            tokio::task::spawn_blocking(move || release_of(&core, &path)).await
        };
        let Ok(Some((mut release, tip))) = read else {
            return;
        };
        let before = core
            .now()
            .reads
            .locked()
            .get(&path)
            .cloned()
            .unwrap_or_default();
        let tip_moved = before.ci_asked.as_deref() != Some(tip.as_str());
        let ask_ci = tip_moved
            || before
                .release
                .as_ref()
                .is_some_and(|kept| kept.releasing || kept.ci == Some(CiStand::Running));
        if ask_ci {
            (release.ci, release.releasing) = ci_of(&core, &path, &release, &tip).await;
        } else if let Some(kept) = &before.release {
            (release.ci, release.releasing) = (kept.ci, kept.releasing);
        }
        let tag_moved = before
            .release
            .as_ref()
            .map(|kept| (&kept.tag, &kept.running))
            != Some((&release.tag, &release.running));
        let going = release.releasing || release.ci == Some(CiStand::Running);
        let changed = before.release.as_ref() != Some(&release);
        core.now().reads.locked().insert(
            path.clone(),
            Read {
                release: Some(release),
                ci_asked: Some(tip),
            },
        );
        if tip_moved || tag_moved {
            let (core, path) = (core.clone(), path.clone());
            let _ = tokio::task::spawn_blocking(move || {
                if tip_moved {
                    super::turn_state::branch_moved(&core, &path);
                }
                if tag_moved {
                    refix_project(&core, &path);
                }
            })
            .await;
        }
        if changed {
            core.changed(What::View);
        }
        if going {
            let weak = Arc::downgrade(&core);
            tokio::spawn(async move {
                tokio::time::sleep(CI_AGAIN).await;
                if let Some(core) = weak.upgrade() {
                    refs_moved(&core, &path);
                }
            });
        }
    });
}

/// The project's newest tag on its branch and what the branch has since, with the branch's tip.
fn release_of(core: &Core, path: &str) -> Option<(NowRelease, String)> {
    let repo = Path::new(path);
    let branch = super::landing::project_branch(repo)?;
    let tip_ref = super::turn_state::landing_tip(core, path, repo)?;
    let tip = Git::quick(repo).trimmed(&["rev-parse", &tip_ref]).ok()?;
    let tag = Git::quick(repo)
        .trimmed(&["describe", "--tags", "--abbrev=0", &tip])
        .ok()
        .filter(|tag| !tag.is_empty());
    let tagged_at = tag.as_deref().and_then(|tag| {
        Git::quick(repo)
            .trimmed(&[
                "for-each-ref",
                "--format=%(creatordate:unix)",
                &format!("refs/tags/{tag}"),
            ])
            .ok()?
            .parse::<u64>()
            .ok()
            .map(|seconds| seconds * 1000)
    });
    let range = match &tag {
        Some(tag) => format!("{tag}..{tip}"),
        None => tip.clone(),
    };
    let since_tag = Git::quick(repo)
        .trimmed(&["rev-list", "--count", &range])
        .ok()
        .and_then(|count| count.parse().ok())
        .unwrap_or(0);
    let running = running_build(core, repo).map(|build| {
        Git::quick(repo)
            .trimmed(&["tag", "--points-at", &build])
            .ok()
            .and_then(|tags| tags.lines().last().map(str::to_owned))
            .unwrap_or(build)
    });
    let release = NowRelease {
        branch,
        tag,
        tagged_at,
        since_tag,
        ci: None,
        releasing: false,
        running,
    };
    Some((release, tip))
}

/// The commit the app the person runs was built at, while that commit is in `repo`.
fn running_build(core: &Core, repo: &Path) -> Option<String> {
    let commit = &core.build().commit;
    let known = !commit.is_empty()
        && commit.chars().all(|c| c.is_ascii_hexdigit())
        && Git::quick(repo)
            .output(&["cat-file", "-e", &format!("{commit}^{{commit}}")])
            .is_ok_and(|output| output.status.success());
    known.then(|| commit.clone())
}

/// How CI stands on the branch's tip, and whether a run on its newest tag is going, as its code
/// host says; none of either where it cannot be read.
async fn ci_of(
    core: &Arc<Core>,
    path: &str,
    release: &NowRelease,
    tip: &str,
) -> (Option<CiStand>, bool) {
    use super::codehost::repo::{self as codehost, Checks};
    let Ok(repo) = codehost::of_project(core, path).await else {
        return (None, false);
    };
    let ci = match codehost::commit_checks(&repo, Some(&release.branch), tip, &[]).await {
        Ok(Checks::Passed) => Some(CiStand::Passed),
        Ok(Checks::Running { .. }) => Some(CiStand::Running),
        Ok(Checks::Failed(_)) => Some(CiStand::Failed),
        Ok(Checks::None | Checks::Stopped(_)) | Err(_) => None,
    };
    let releasing = match &release.tag {
        Some(tag) => codehost::runs(&repo, tag, None, false)
            .await
            .is_ok_and(|runs| {
                runs.iter().any(|run| {
                    run["status"]
                        .as_str()
                        .is_some_and(|status| status != "completed")
                })
            }),
        None => false,
    };
    (ci, releasing)
}

/// Where `agent`'s landed work is, read again: on a land, a push or a new turn of it.
pub(crate) fn work_moved(core: &Core, agent: &str) {
    let fix = fix_of(core, agent);
    let changed = {
        let mut fixes = core.now().fixes.locked();
        match fix {
            Some(fix) => fixes.insert(agent.to_owned(), fix.clone()) != Some(fix),
            None => fixes.remove(agent).is_some(),
        }
    };
    if changed {
        core.changed(What::View);
    }
    asks::work_read(core, agent);
}

/// Every agent's fix in the project at `path`, read again since its newest tag or the app's
/// build moved.
fn refix_project(core: &Core, path: &str) {
    let Some(project) = (core.workspaces().projects().into_iter())
        .find(|project| project.path.to_string_lossy() == path)
    else {
        return;
    };
    let agents = core.workspaces().agent_projects();
    for (agent, _) in agents.iter().filter(|(_, of)| **of == project.id) {
        work_moved(core, agent);
    }
    asks::build_moved(core, &project.id, &project.path);
}

/// How far `agent`'s landed work has gone, as last read.
fn fix_now(core: &Core, agent: &str) -> Option<Fix> {
    core.now().fixes.locked().get(agent).cloned()
}

fn fix_of(core: &Core, agent: &str) -> Option<Fix> {
    let work = core.inbox().ending(agent)?.work?;
    if work.changes != WorkAt::Landed {
        return None;
    }
    let repo_path = (core.workspaces().agent(agent))
        .and_then(|agent| agent.worktree.map(|worktree| worktree.repo))
        .unwrap_or_else(|| PathBuf::from(&work.project));
    let repo = repo_path.as_path();
    let commit = (work.own.first())
        .and_then(|own| own.split_whitespace().next().map(str::to_owned))
        .or(work.head)?;
    let contains = |tip: &str| {
        Git::quick(repo)
            .output(&["merge-base", "--is-ancestor", &commit, tip])
            .is_ok_and(|output| output.status.success())
    };
    if running_build(core, repo).is_some_and(|build| contains(&build)) {
        return Some(Fix {
            commit,
            at: FixAt::Running,
            release: None,
        });
    }
    let kept = core.now().fixes.locked().get(agent).cloned();
    let newest = (core.now().reads.locked().get(&work.project))
        .and_then(|read| read.release.as_ref()?.tag.clone());
    let tag = match kept.and_then(|kept| (kept.commit == commit).then_some(kept.release)?) {
        Some(tag) => Some(tag),
        None => newest.filter(|newest| contains(newest)).map(|newest| {
            Git::quick(repo)
                .trimmed(&["describe", "--tags", "--contains", &commit])
                .ok()
                .and_then(|name| name.split(['~', '^']).next().map(str::to_owned))
                .filter(|tag| !tag.is_empty())
                .unwrap_or(newest)
        }),
    };
    Some(Fix {
        commit,
        at: match tag {
            Some(_) => FixAt::Released,
            None => FixAt::Landed,
        },
        release: tag,
    })
}
