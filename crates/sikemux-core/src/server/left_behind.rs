//! Work an agent leaves behind: commits that never landed, and changes it never committed.
//! Whoever hears of an idle agent ([`roster::hears_of`]) is woken for them a few minutes after
//! its turn ends, once per idle stretch; with only the person to hear, the agent's row says it
//! already. A lead closing a worker hears in `agent_close`'s answer what the close left, and a
//! project's chief hears of the person's asks that nobody has.

use crate::server::agent_now::AgentNowCore;
use crate::server::parts::Parts;
use crate::server::workspace_state::WorkspaceParts;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use sikemux_view::turn::{TurnPhase, TurnState, WorkAt};
use sikemux_view::workspace::WorkspaceAgent;

use super::roster::{self, Hears};
use super::Core;
use crate::git::Git;

const GRACE_MS: u64 = 5 * 60 * 1000;
const MOST_LISTED: usize = 10;
const FILES_LISTED: usize = 5;

pub(crate) struct Left {
    agent: String,
    to: String,
    text: String,
}

/// The idle agents whose left work is due to be told of now, and when the next one falls due.
/// `told` keeps, per agent, the end of the turn it was last told about.
pub(crate) fn read(
    core: &Core,
    turns: &HashMap<String, TurnState>,
    told: &mut HashMap<String, u64>,
    now: u64,
) -> (Vec<Left>, Option<u64>) {
    told.retain(|agent, _| core.workspaces().agent(agent).is_some());
    let mut left = Vec::new();
    let mut next: Option<u64> = None;
    for agent in core.workspaces().agents() {
        let Some(turn) = turns.get(&agent.id) else {
            continue;
        };
        let Some(changes) = idle_with_work(core, &agent.id, turn) else {
            continue;
        };
        if told.get(&agent.id) == Some(&turn.since) {
            continue;
        }
        let due = turn.since + GRACE_MS;
        if now < due {
            next = Some(next.map_or(due, |next| next.min(due)));
            continue;
        }
        let hears = roster::hears_of(core, &agent.id);
        let Some(to) = hears.agent().map(str::to_owned) else {
            continue;
        };
        told.insert(agent.id.clone(), turn.since);
        let text = match changes {
            WorkAt::Committed => unlanded_text(core, &agent, &hears),
            _ => uncommitted_text(core, &agent, &hears),
        };
        if let Some(text) = text {
            left.push(Left {
                agent: agent.id,
                to,
                text,
            });
        }
    }
    (left, next)
}

pub(crate) fn tell(core: &Arc<Core>, left: Vec<Left>) {
    for left in left {
        super::messages::report_left(core, left.agent, left.to, left.text);
    }
}

/// A note for a project's chief on the person's asks nobody has.
pub(crate) struct Unowned {
    chief: String,
    text: String,
}

/// The person's asks that have had nobody on them for the grace, grouped by the chief of their
/// project, told once per stretch with nobody; and when the next one falls due.
pub(crate) fn asks(
    core: &Core,
    told: &mut HashMap<u64, u64>,
    now: u64,
) -> (Vec<Unowned>, Option<u64>) {
    let nobody = super::now::asks::with_nobody(core);
    told.retain(|id, since| nobody.iter().any(|ask| ask.0 == *id && ask.3 == *since));
    let chiefs = super::chief::seated(core);
    let mut lines: HashMap<String, Vec<String>> = HashMap::new();
    let mut next: Option<u64> = None;
    for (id, project, title, since) in nobody {
        if told.contains_key(&id) {
            continue;
        }
        let Some(chief) = chiefs.get(&project) else {
            continue;
        };
        let due = since + GRACE_MS;
        if now < due {
            next = Some(next.map_or(due, |next| next.min(due)));
            continue;
        }
        told.insert(id, since);
        (lines.entry(chief.clone()).or_default()).push(format!(
            "- {title} ({} min)",
            now.saturating_sub(since) / 60_000
        ));
    }
    let notes = (lines.into_iter())
        .map(|(chief, lines)| Unowned {
            chief,
            text: format!(
                "{} of the person's asks {} had nobody on them for over {} minutes:\n{}\n\nRoute each to a worker, or say why it waits. You hear of each once until someone takes it.",
                lines.len(),
                if lines.len() == 1 { "has" } else { "have" },
                GRACE_MS / 60_000,
                lines.join("\n"),
            ),
        })
        .collect();
    (notes, next)
}

pub(crate) fn tell_chiefs(core: &Arc<Core>, notes: Vec<Unowned>) {
    for note in notes {
        let core = core.clone();
        tokio::spawn(async move {
            let why = "because asks of the person's have nobody on them";
            if let Err(error) =
                super::messages::work_from_sikemux(&core, note.chief, note.text, why).await
            {
                eprintln!("sikemux core: the chief did not hear of asks with nobody: {error}");
            }
        });
    }
}

fn idle_with_work(core: &Core, agent_id: &str, turn: &TurnState) -> Option<WorkAt> {
    let changes = (turn.state == TurnPhase::Done && turn.call.is_none())
        .then(|| turn.done.as_ref().map(|done| done.changes))
        .flatten()
        .filter(|changes| matches!(changes, WorkAt::Committed | WorkAt::Uncommitted))?;
    let now = core.agent_now(agent_id);
    let waits =
        now.at_work() || now.waits_on_something() || super::turn_state::set_a_wake(core, agent_id);
    (!waits).then_some(changes)
}

fn opening(hears: &Hears) -> &'static str {
    match hears {
        Hears::Chief(_) => {
            "No lead of its is open, so this comes to you as the project's chief. It"
        }
        _ => "It",
    }
}

fn unlanded_text(core: &Core, agent: &WorkspaceAgent, hears: &Hears) -> Option<String> {
    let (commits, branch) = commits_off(core, agent)?;
    let mut list: Vec<String> = (commits.iter().take(MOST_LISTED))
        .map(|commit| format!("- {commit}"))
        .collect();
    if commits.len() > MOST_LISTED {
        list.push(format!("- and {} more", commits.len() - MOST_LISTED));
    }
    Some(format!(
        "{} has been idle for {} minutes with {} off {branch}:\n{}\n\nLand what is listed (ask it to run git_commit with land: true, or land it yourself), or close it if that work is not wanted. You hear this once until it works again.",
        opening(hears),
        GRACE_MS / 60_000,
        counted(commits.len(), "a commit", "commits"),
        list.join("\n"),
    ))
}

fn uncommitted_text(core: &Core, agent: &WorkspaceAgent, hears: &Hears) -> Option<String> {
    let files = uncommitted(core, agent);
    if files.is_empty() {
        return None;
    }
    let mut list: Vec<String> = (files.iter().take(FILES_LISTED))
        .map(|file| format!("- {file}"))
        .collect();
    if files.len() > FILES_LISTED {
        list.push(format!("- and {} more", files.len() - FILES_LISTED));
    }
    Some(format!(
        "{} has been idle for {} minutes with {} it never committed:\n{}\n\nAsk it to commit and land them (git_commit with land: true), or to drop them and close if that work is not wanted. You hear this once until it works again.",
        opening(hears),
        GRACE_MS / 60_000,
        counted(files.len(), "a changed file", "changed files"),
        list.join("\n"),
    ))
}

/// What closing `agent_id` leaves with nobody to hear of it, in a line for the lead closing it.
pub(crate) fn left_by_close(core: &Core, agent_id: &str, name: &str) -> Option<String> {
    let agent = core.workspaces().agent(agent_id)?;
    let committed = super::turn_state::now(core, agent_id)
        .and_then(|turn| turn.done)
        .is_some_and(|done| done.changes == WorkAt::Committed);
    let mut parts = Vec::new();
    if committed || agent.worktree.is_some() {
        if let Some((commits, branch)) = commits_off(core, &agent) {
            parts.push(format!(
                "{} off {branch}",
                counted(commits.len(), "a commit", "commits")
            ));
        }
    }
    let files = uncommitted(core, &agent);
    if !files.is_empty() {
        parts.push(counted(
            files.len(),
            "an uncommitted file",
            "uncommitted files",
        ));
    }
    let triggers = core.triggers().titles(agent_id);
    if !triggers.is_empty() {
        let titles: Vec<String> = triggers
            .iter()
            .map(|title| format!("\"{title}\""))
            .collect();
        parts.push(format!("its triggers {}", titles.join(", ")));
    }
    if parts.is_empty() {
        return None;
    }
    let last = parts.pop()?;
    let listed = match parts.is_empty() {
        true => last,
        false => format!("{} and {last}", parts.join(", ")),
    };
    Some(format!(
        "{name} left {listed}, and nothing tells anyone of them now."
    ))
}

fn counted(count: usize, one: &str, many: &str) -> String {
    match count {
        1 => one.to_owned(),
        count => format!("{count} {many}"),
    }
}

fn uncommitted(core: &Core, agent: &WorkspaceAgent) -> Vec<String> {
    let Some(named) = core.names().get(&agent.id) else {
        return Vec::new();
    };
    super::inbox::uncommitted_files(core, &named.project, &agent.id, &agent.cwd)
}

/// The agent's commits not on the branch it lands on, newest first, with that branch.
fn commits_off(core: &Core, agent: &WorkspaceAgent) -> Option<(Vec<String>, String)> {
    let project = core.workspaces().project_path(&agent.project).ok()?;
    let repo: PathBuf = (agent.worktree.as_ref()).map_or(project.clone(), |tree| tree.repo.clone());
    let tip = super::turn_state::landing_tip(core, &project.to_string_lossy(), &repo)?;
    let listed = Git::quick(&agent.cwd)
        .text(&[
            "log",
            "--cherry-pick",
            "--right-only",
            "--no-merges",
            "--format=%h %s",
            &format!("{tip}...HEAD"),
        ])
        .ok()?;
    let commits: Vec<String> = (listed.lines())
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect();
    if commits.is_empty() {
        return None;
    }
    let branch = (tip.strip_prefix("refs/heads/"))
        .or_else(|| tip.strip_prefix("refs/remotes/"))
        .unwrap_or(&tip)
        .to_owned();
    Some((commits, branch))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::activity::{Activity, ActivityCore};
    use crate::server::names::{Starting, Worker};
    use crate::server::turn_state::Work;
    use crate::server::workspace::{AddProjectParams, AddProjectParamsExt};
    use crate::server::ServerCore;
    use serde_json::{json, Value};
    use std::collections::HashSet;
    use std::path::Path;

    fn git(dir: &Path, args: &[&str]) {
        let mut all = vec!["-c", "user.name=test", "-c", "user.email=test@localhost"];
        all.extend(args);
        Git::at(dir).text(&all).expect("git");
    }

    async fn project(core: &Arc<Core>, dir: &Path) -> sikemux_view::workspace::ProjectInfo {
        git(dir, &["init", "-q", "-b", "main"]);
        git(dir, &["commit", "-q", "--allow-empty", "-m", "start"]);
        crate::server::workspace::add_project(core, AddProjectParams::at(dir.to_owned()))
            .await
            .expect("the project")
    }

    fn open(core: &Core, id: &str, project: &str, cwd: &Path, lead: Option<&str>) {
        let Value::Object(agent) = json!({
            "id": id,
            "provider": "claude",
            "project": project,
            "cwd": cwd,
            "title": id,
            "lead": lead,
            "permissionMode": "auto",
        }) else {
            unreachable!()
        };
        core.workspaces().put_agent_draft(agent).expect("the agent");
    }

    fn name(core: &Core, id: &str, path: &str, lead: Option<&str>) {
        match lead {
            Some(lead) => {
                let worker = Worker {
                    agent_id: id,
                    lead,
                    project: None,
                    provider: "claude",
                    session: None,
                    reports: true,
                    task: None,
                };
                core.names().hire(worker, HashSet::new()).expect("named");
            }
            None => {
                let starting = Starting {
                    agent_id: id,
                    project: path.to_owned(),
                    provider: None,
                    resume_id: None,
                };
                core.names().give(starting, HashSet::new());
            }
        }
    }

    /// @Roswaal's lead closed and @Hazamada was started by the person, so their left work reached
    /// nobody, while a stall reached only a lead that was awake.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_lead_hears_while_open_asleep_or_not_then_the_chief() {
        let core = Core::new(Default::default(), None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let project = project(&core, dir.path()).await;
        let path = project.path.to_string_lossy().into_owned();
        for (id, lead) in [
            ("chief", None),
            ("lead", None),
            ("worker", Some("lead")),
            ("solo", None),
        ] {
            name(&core, id, &path, lead);
            open(&core, id, &project.id, dir.path(), lead);
        }
        core.workspaces()
            .set_chief(&project.id, "chief", 1)
            .expect("the chief");

        assert_eq!(
            roster::hears_of(&core, "worker"),
            Hears::Lead("lead".into())
        );
        assert_eq!(
            roster::hears_of(&core, "solo"),
            Hears::Chief("chief".into())
        );
        assert_eq!(roster::hears_of(&core, "chief"), Hears::Person);

        crate::server::workspace::close(&core, &["lead".to_owned()]).expect("closed");
        assert_eq!(
            roster::hears_of(&core, "worker"),
            Hears::Chief("chief".into())
        );
    }

    /// On 15 Nov five workers were closed by their leads with commits off main, and nothing
    /// said so after.
    #[tokio::test(flavor = "multi_thread")]
    async fn closing_a_worker_with_commits_off_the_branch_says_so() {
        let core = Core::new(Default::default(), None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let project = project(&core, dir.path()).await;
        let tree = dir.path().join("worker-tree");
        git(
            dir.path(),
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "work",
                tree.to_str().unwrap(),
            ],
        );
        git(
            &tree,
            &["commit", "-q", "--allow-empty", "-m", "the worker's change"],
        );
        let path = project.path.to_string_lossy().into_owned();
        name(&core, "lead", &path, None);
        name(&core, "worker", &path, Some("lead"));
        open(&core, "worker", &project.id, &tree, Some("lead"));
        assert_eq!(left_by_close(&core, "worker", "@Worker"), None);

        core.note_activity("worker", Activity::Started);
        core.note_activity("worker", Activity::Finished { cancelled: false });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while (core.inbox().ending("worker")).is_none_or(|ended| ended.work.is_none()) {
            assert!(
                std::time::Instant::now() < deadline,
                "the work was never read"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        core.inbox().keep_work(
            "worker",
            Work {
                changes: WorkAt::Committed,
                head: None,
                head_at: None,
                project: path,
                can_try: false,
                own: Vec::new(),
                shared: 0,
            },
        );

        assert_eq!(
            left_by_close(&core, "worker", "@Worker").as_deref(),
            Some("@Worker left a commit off main, and nothing tells anyone of them now.")
        );
    }
}
