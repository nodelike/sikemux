//! The worktree a chat agent or a worker is given, and the setup the project
//! asks for in it.

use crate::server::changed::ChangedCore;
use crate::server::parts::Parts;
use crate::server::workspace_state::WorkspaceParts;
pub use sikemux_calls::worktree::{AgentWorktreeParams, MadeAgentWorktree};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;
use sikemux_process::vars;
use sikemux_view::changed::Topic;
use sikemux_view::host_stats::HostEventKind;

use super::{
    add_again, branch_exists, branch_for, copy_included, cut, inside, warm, worktrees_folder,
    AgentWorktree,
};
use crate::git::{main_checkout, Git};
use crate::server::changed::What;
use crate::server::connection::blocking;
use crate::server::ports::checkout;
use crate::server::{launching, tasks, Core, CoreResult};
use crate::tasks::config::{self, ConfigLoad, WorktreeHook};

pub(crate) const NOT_IN_GIT: &str = "is not in a git repository";

/// A worktree of the repository `checkout` is in, cut from where it is now on
/// the branch `branch`, with what it needs copied in, and the folder in it
/// that stands where `cwd` stood.
pub(crate) fn cut_for(
    checkout: &Path,
    cwd: &Path,
    branch: &str,
) -> Result<(AgentWorktree, PathBuf), String> {
    let repo = PathBuf::from(
        Git::quick(checkout)
            .trimmed(&["rev-parse", "--path-format=absolute", "--show-toplevel"])
            .map_err(|_| format!("{} {NOT_IN_GIT}", checkout.display()))?,
    );
    let main = main_checkout(&repo).unwrap_or_else(|| repo.clone());
    let start_sha = Git::quick(&repo)
        .trimmed(&["rev-parse", "--verify", "HEAD"])
        .map_err(|_| "the project has no commit to branch from")?;
    let base = Git::quick(&repo)
        .trimmed(&["symbolic-ref", "--quiet", "--short", "HEAD"])
        .ok();
    let (branch, path) = cut(&repo, branch, &start_sha)?;
    copy_included(&main, &path);
    warm::seed_in_background(&main, &path);
    let inside = if cwd.starts_with(&repo) {
        inside(cwd, &repo)
    } else {
        PathBuf::new()
    };
    Ok((
        AgentWorktree {
            repo: main,
            path: path.clone(),
            branch,
            base,
            start_sha,
            name: String::new(),
            first_port: None,
        },
        path.join(inside),
    ))
}

/// A worktree cut for an agent that has not started yet, removed again if
/// starting it fails, so no branch or folder is left behind.
pub(crate) struct Unclaimed(pub Option<AgentWorktree>);

impl Drop for Unclaimed {
    fn drop(&mut self) {
        if let Some(tree) = self.0.take() {
            super::remove(&tree.repo, &tree.path, &tree.branch);
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct AtParams {
    cwd: PathBuf,
}

/// The worktree the core cut that `cwd` lies in, read back from git, for a
/// saved chat that resumes there. The project's current branch stands in as
/// its base.
pub(crate) async fn at(params: AtParams) -> CoreResult<Option<AgentWorktree>> {
    blocking(move || Ok(read_back(&params.cwd))).await
}

/// Where an agent that ran in `cwd` takes its saved session up again, and its own checkout there.
pub(crate) struct ResumedIn {
    pub cwd: PathBuf,
    pub worktree: Option<AgentWorktree>,
    /// Whether the worktree was cleaned away and is made again, so it needs the project's setup.
    pub made_again: bool,
}

/// Where an agent that ran in `cwd` resumes: there, making its worktree again at the same path
/// when it was cleaned away, since Claude and Codex find a session by the folder it ran in.
/// `branch` is the one the agent's record names, when it has one.
pub(crate) fn resumed_in(cwd: &Path, branch: Option<&str>) -> Result<ResumedIn, String> {
    if cwd.is_dir() {
        return Ok(ResumedIn {
            worktree: read_back(cwd),
            cwd: cwd.to_owned(),
            made_again: false,
        });
    }
    let worktree = make_again(cwd, branch).map_err(|why| {
        format!(
            "the folder the session ran in, {}, is gone, and {why}",
            cwd.display()
        )
    })?;
    Ok(ResumedIn {
        cwd: cwd.to_owned(),
        worktree: Some(worktree),
        made_again: true,
    })
}

/// Makes again the worktree a stored agent ran in when it was cleaned away, so it wakes on its
/// session there, and keeps it on the agent's record. One still there gets back, warm, any
/// build output that was removed while the agent was closed.
pub(crate) async fn restore(core: &Arc<Core>, agent_id: &str) -> CoreResult<()> {
    let agent = core.workspaces().agent_or_gone(agent_id)?;
    if agent.cwd.is_dir() {
        if let Some(tree) = agent.worktree {
            warm::seed_in_background(&tree.repo, &tree.path);
        }
        return Ok(());
    }
    let (cwd, branch) = (agent.cwd, agent.worktree.map(|tree| tree.branch));
    let mut resumed = blocking(move || Ok(resumed_in(&cwd, branch.as_deref())?)).await?;
    if let Some(tree) = resumed.worktree.as_mut() {
        checkout::claim(core, tree);
    }
    core.workspaces()
        .edit_agent(agent_id, |agent| agent.worktree = resumed.worktree.clone())?;
    set_up_made_again(core, agent_id, &resumed);
    Ok(())
}

/// Runs the project's worktree setup in a worktree made again for `agent_id`, without holding
/// up its start.
pub(crate) fn set_up_made_again(core: &Arc<Core>, agent_id: &str, resumed: &ResumedIn) {
    let Some(tree) = resumed.worktree.as_ref().filter(|_| resumed.made_again) else {
        return;
    };
    let (core, agent_id) = (core.clone(), agent_id.to_owned());
    let (repo, cwd) = (tree.repo.clone(), resumed.cwd.clone());
    tokio::spawn(async move {
        if let Some(error) = set_up(&core, &repo, Some(&agent_id), &cwd).await {
            eprintln!(
                "sikemux core: {agent_id}'s worktree made again is not fully set up: {error}"
            );
        }
    });
}

/// Makes the worktree that held `cwd` again at the same path: on its branch while that has work
/// the project's branch lacks, else on the branch reset to where the project is now.
fn make_again(cwd: &Path, branch: Option<&str>) -> Result<AgentWorktree, String> {
    let (repo, path) = lost_worktree(cwd)
        .ok_or("it was not a worktree beside a repository, so it cannot be made again")?;
    let folder = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let base = Git::quick(&repo)
        .trimmed(&["symbolic-ref", "--quiet", "--short", "HEAD"])
        .ok();
    let now = Git::quick(&repo)
        .trimmed(&["rev-parse", "--verify", "HEAD"])
        .map_err(|_| "the project has no commit to make it from")?;
    let kept = branch
        .map(str::to_owned)
        .filter(|branch| branch_exists(&repo, branch))
        .or_else(|| branch_for_folder(&repo, &folder));
    let (branch, start_sha, start) = match kept {
        Some(branch) if !landed(&repo, &branch, &now) => {
            let start_sha = Git::quick(&repo).trimmed(&["merge-base", &now, &branch])?;
            (branch.clone(), start_sha, branch)
        }
        Some(branch) => (branch, now.clone(), now),
        None => {
            let branch = branch.map_or(folder, str::to_owned);
            (branch, now.clone(), now)
        }
    };
    add_again(&repo, &path, &branch, &start)?;
    Ok(AgentWorktree {
        repo,
        path,
        branch,
        base,
        start_sha,
        name: String::new(),
        first_port: None,
    })
}

/// Whether every commit of `branch` is in `onto` already, as it is or replayed on top.
fn landed(repo: &Path, branch: &str, onto: &str) -> bool {
    Git::quick(repo)
        .trimmed(&["cherry", onto, branch])
        .is_ok_and(|unlanded| !unlanded.lines().any(|line| line.starts_with('+')))
}

/// The main checkout and the worktree folder in its worktrees folder that `cwd` lay in, while
/// that repository is still there.
fn lost_worktree(cwd: &Path) -> Option<(PathBuf, PathBuf)> {
    cwd.ancestors().find_map(|path| {
        let folder = path.parent()?;
        let name = folder.file_name()?.to_str()?.strip_suffix(".worktrees")?;
        let repo = folder.with_file_name(name);
        (main_checkout(&repo).as_deref() == Some(repo.as_path())).then(|| (repo, path.to_owned()))
    })
}

/// The local branch whose worktree folder is named `folder`, as [`super::cut`] names them.
fn branch_for_folder(repo: &Path, folder: &str) -> Option<String> {
    Git::quick(repo)
        .trimmed(&["for-each-ref", "--format=%(refname:short)", "refs/heads"])
        .ok()?
        .lines()
        .find(|branch| branch.replace('/', "-") == folder)
        .map(str::to_owned)
}

fn read_back(cwd: &Path) -> Option<AgentWorktree> {
    let path = PathBuf::from(
        Git::quick(cwd)
            .trimmed(&["rev-parse", "--path-format=absolute", "--show-toplevel"])
            .ok()?,
    );
    let main = main_checkout(&path)?;
    if !path.starts_with(worktrees_folder(&main)) {
        return None;
    }
    let branch = Git::quick(&path)
        .trimmed(&["symbolic-ref", "--quiet", "--short", "HEAD"])
        .ok()?;
    let base = Git::quick(&main)
        .trimmed(&["symbolic-ref", "--quiet", "--short", "HEAD"])
        .ok();
    let start_sha = Git::quick(&main)
        .trimmed(&["merge-base", "HEAD", &branch])
        .ok()?;
    Some(AgentWorktree {
        repo: main,
        path,
        branch,
        base,
        start_sha,
        name: String::new(),
        first_port: None,
    })
}

/// Gives an agent a worktree of its own cut from where its checkout is now,
/// moves it there, and starts it again there if it was running.
pub(crate) async fn for_agent(
    core: &Arc<Core>,
    params: AgentWorktreeParams,
) -> CoreResult<MadeAgentWorktree> {
    let agent = core
        .workspaces()
        .view()
        .agents
        .into_iter()
        .find(|agent| agent.id == params.agent_id)
        .ok_or("that agent is not on this host")?;
    if agent.worktree.is_some() {
        return Err("that agent already works in a worktree of its own".into());
    }
    let cwd = agent.cwd.clone();
    let checkout = params.repo.unwrap_or_else(|| cwd.clone());
    let branch = branch_for(&params.message);
    let (mut worktree, tree_cwd) = blocking(move || Ok(cut_for(&checkout, &cwd, &branch)?)).await?;
    checkout::claim(core, &mut worktree);
    let project = core
        .workspaces()
        .view()
        .projects
        .into_iter()
        .find(|project| project.id == agent.project)
        .map_or(agent.cwd, |project| project.path);
    let setup_error = set_up(core, &project, Some(&params.agent_id), &tree_cwd).await;
    core.workspaces().edit_agent(&params.agent_id, |agent| {
        agent.worktree = Some(worktree.clone());
        agent.cwd = tree_cwd.clone();
    })?;
    if core.chats().stop(&params.agent_id) {
        launching::start_agent(core, None, &params.agent_id, false).await?;
    }
    Ok(MadeAgentWorktree {
        worktree,
        cwd: tree_cwd,
        setup_error,
    })
}

/// The checkout of `project`'s repository that commits are built and checked
/// in, apart from the work in progress in everyone's own trees, moved to
/// `commit`. It stays at one place beside the repository, so each build after
/// the first only redoes what the commits changed.
pub(crate) async fn integration(
    core: &Arc<Core>,
    project: &Path,
    commit: &str,
) -> Result<PathBuf, String> {
    clean_at(core, project, commit, INTEGRATION).await
}

/// The checkout of `project`'s repository that assertions' commands run in, moved to `commit`. It
/// is not the integration checkout, which a long command would otherwise move under a running check.
pub(crate) async fn gates(
    core: &Arc<Core>,
    project: &Path,
    commit: &str,
) -> Result<PathBuf, String> {
    clean_at(core, project, commit, GATES).await
}

/// The checkout of `project`'s repository the merge queue stacks changes and
/// checks them in, so a check of the branch never has it moved from under it.
pub(crate) async fn merge_queue(
    core: &Arc<Core>,
    project: &Path,
    commit: &str,
) -> Result<PathBuf, String> {
    clean_at(core, project, commit, MERGE_QUEUE).await
}

/// The checkout of `project`'s repository that its reservoir command runs in, moved to `commit`.
/// It is its own, as that command can run for as long as a check does.
pub(crate) async fn reservoir(
    core: &Arc<Core>,
    project: &Path,
    commit: &str,
) -> Result<PathBuf, String> {
    clean_at(core, project, commit, RESERVOIR).await
}

async fn clean_at(
    core: &Arc<Core>,
    project: &Path,
    commit: &str,
    folder: &'static str,
) -> Result<PathBuf, String> {
    let (repo, commit) = (project.to_path_buf(), commit.to_owned());
    let (top, path, fresh) = blocking(move || {
        let top = PathBuf::from(Git::quick(&repo).trimmed(&[
            "rev-parse",
            "--path-format=absolute",
            "--show-toplevel",
        ])?);
        let path = worktrees_folder(&top).join(folder);
        let fresh = !path.join(".git").exists();
        let target = path.to_string_lossy().into_owned();
        if fresh {
            let _ = Git::quick(&top).trimmed(&["worktree", "prune"]);
            Git::slow(&top)
                .trimmed(&["worktree", "add", "--detach", "--force", &target, &commit])?;
            copy_included(&top, &path);
            super::warm::seed(&top, &path);
        } else {
            Git::slow(&path).trimmed(&["checkout", "--detach", "--force", "--quiet", &commit])?;
            Git::quick(&path).trimmed(&["clean", "-fdq"])?;
        }
        Ok((top, path, fresh))
    })
    .await
    .map_err(|error| error.to_string())?;
    if fresh {
        if let Some(error) = set_up(core, &top, None, &path).await {
            return Err(error);
        }
    }
    Ok(path.join(inside(project, &top)))
}

/// The integration checkout's folder in the repository's worktrees folder.
const INTEGRATION: &str = ".integration";
/// The gates checkout's folder beside it.
const GATES: &str = ".gates";
/// The merge queue's checkout's folder beside them.
const MERGE_QUEUE: &str = ".merge-queue";
/// The reservoir checkout's folder beside them.
const RESERVOIR: &str = ".reservoir";

/// Runs `project`'s `worktree.onCreate` commands in the new checkout at `cwd`
/// once the person trusts its `sikemux.json`, asked on behalf of `agent_id`
/// when an agent wants the checkout. Why they did not all finish, when they
/// did not.
pub(crate) async fn set_up(
    core: &Arc<Core>,
    project: &Path,
    agent_id: Option<&str>,
    cwd: &Path,
) -> Option<String> {
    let load = {
        let project = project.to_path_buf();
        blocking(move || Ok(config::load(&project))).await.ok()?
    };
    let hooks = match load {
        ConfigLoad::Absent { .. } => return None,
        ConfigLoad::Invalid { errors, .. } => {
            let first = errors
                .first()
                .map_or("invalid", |error| error.message.as_str());
            return Some(format!(
                "sikemux.json needs attention, so the worktree setup did not run: {first}"
            ));
        }
        ConfigLoad::Valid { config, .. } => config
            .worktree
            .map(|worktree| worktree.on_create)
            .unwrap_or_default(),
    };
    if hooks.is_empty() {
        return None;
    }
    if !tasks::config_trusted(core, project, agent_id).await {
        return Some(
            "The worktree setup in sikemux.json was not trusted, so it did not run".into(),
        );
    }
    run_hooks(hooks, cwd, environment(core, cwd).await)
        .await
        .map(|failed| format!("{} failed: {}", failed.label, failed.output))
}

/// Runs `project`'s `worktree.onSettle` commands in the agent worktree at `tree` once nothing
/// works there any more. They run only when the person already trusts its `sikemux.json`, as no
/// one is there to ask.
pub(crate) async fn settle(core: Arc<Core>, project: PathBuf, tree: PathBuf) {
    let load = {
        let project = project.clone();
        blocking(move || Ok(config::load(&project))).await
    };
    let Ok(load) = load else {
        return;
    };
    if tasks::config_needs(&core, &load).is_some() {
        return;
    }
    let ConfigLoad::Valid { config, .. } = load else {
        return;
    };
    let hooks = config
        .worktree
        .map(|worktree| worktree.on_settle)
        .unwrap_or_default();
    if hooks.is_empty() || !tree.is_dir() {
        return;
    }
    let env = environment(&core, &tree).await;
    let Some(failed) = run_hooks(hooks, &tree, env).await else {
        return;
    };
    let worktree = tree.file_name().map_or_else(
        || tree.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    core.host_watch().note(HostEventKind::SettleFailed {
        worktree,
        command: failed.label,
        output: failed.output,
    });
    core.changed(What::Read(Topic::HostStats));
}

/// A hook that failed: its label, and the end of what it printed or why it could not run.
pub(crate) struct Failed {
    pub label: String,
    pub output: String,
}

/// What the checkout at `cwd` gives the steps run in it.
async fn environment(core: &Arc<Core>, cwd: &Path) -> Vec<(String, String)> {
    let (core, cwd) = (core.clone(), cwd.to_owned());
    blocking(move || Ok(checkout::here(&core, &cwd).map(|here| here.environment())))
        .await
        .ok()
        .flatten()
        .unwrap_or_default()
}

/// How much of the end of a failed hook's output is kept.
const FAILED_LINES: usize = 3;

/// Runs `hooks` in `cwd` with `env` one after another, stopping at the first that fails.
pub(crate) async fn run_hooks(
    hooks: Vec<WorktreeHook>,
    cwd: &Path,
    env: Vec<(String, String)>,
) -> Option<Failed> {
    let mut env = env;
    env.push((vars::PROJECT.to_owned(), cwd.to_string_lossy().into_owned()));
    for hook in hooks {
        let (command, cwd, env) = (hook.command.clone(), cwd.to_owned(), env.clone());
        let ran = blocking(move || {
            let log =
                std::env::temp_dir().join(format!("sikemux-step-{}.log", uuid::Uuid::new_v4()));
            let ran = crate::server::setup_check::shell(&command, &cwd, &env, &log);
            let printed = std::fs::read_to_string(&log).unwrap_or_default();
            let _ = std::fs::remove_file(&log);
            Ok((ran, printed))
        })
        .await;
        let output = match ran {
            Ok((Ok(()), _)) => continue,
            Ok((Err(why), printed)) => match printed.trim() {
                "" => why,
                printed => {
                    let lines: Vec<&str> = printed.lines().collect();
                    lines[lines.len().saturating_sub(FAILED_LINES)..].join("\n")
                }
            },
            Err(error) => error.to_string(),
        };
        return Some(Failed {
            label: hook.label,
            output,
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_resumes_in_its_worktree_made_again_where_it_was() {
        let parent = tempfile::tempdir().unwrap();
        let repo = parent.path().join("app");
        std::fs::create_dir(&repo).unwrap();
        let commit = ["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q"];
        Git::quick(&repo).trimmed(&["init", "-q"]).unwrap();
        Git::quick(&repo)
            .trimmed(&[&commit[..], &["--allow-empty", "-m", "base"]].concat())
            .unwrap();
        let (branch, path) = cut(&repo, "fix/lost", "HEAD").unwrap();
        Git::quick(&path)
            .trimmed(&[&commit[..], &["--allow-empty", "-m", "work"]].concat())
            .unwrap();
        let work = Git::quick(&path).trimmed(&["rev-parse", "HEAD"]).unwrap();
        std::fs::remove_dir_all(&path).unwrap();

        let resumed = resumed_in(&path, None).unwrap();
        assert!(resumed.made_again);
        assert_eq!(resumed.cwd, path);
        let tree = resumed.worktree.unwrap();
        assert_eq!(tree.branch, branch);
        assert_eq!(
            Git::quick(&path).trimmed(&["rev-parse", "HEAD"]).unwrap(),
            work
        );

        let gone = parent.path().join("elsewhere");
        let error = resumed_in(&gone, None).err().unwrap();
        assert!(error.contains(&gone.display().to_string()));
    }
}
