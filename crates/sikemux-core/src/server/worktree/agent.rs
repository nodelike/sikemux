//! The worktree a chat agent or a worker is given, and the setup the project
//! asks for in it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sikemux_process::vars;

use super::{copy_included, cut, inside, warm, worktrees_folder, AgentWorktree};
use crate::git::{main_checkout, Git};
use crate::server::connection::blocking;
use crate::server::custom_command::{self, CommandParams};
use crate::server::{launching, tasks, Core, CoreResult};
use crate::tasks::config::{self, ConfigLoad};

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForAgentParams {
    pub agent_id: String,
    /// What the agent was asked first, which names the branch.
    pub message: String,
    /// The repository to use when the agent's folder holds several.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<PathBuf>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForAgent {
    pub worktree: AgentWorktree,
    pub cwd: PathBuf,
    /// Why the project's worktree setup did not all finish, when it did not.
    pub setup_error: Option<String>,
}

/// A worktree of the repository `checkout` is in, cut from where it is now on
/// a branch named for `name`, with what it needs copied in, and the folder in
/// it that stands where `cwd` stood.
pub(crate) fn cut_for(
    checkout: &Path,
    cwd: &Path,
    name: &str,
) -> Result<(AgentWorktree, PathBuf), String> {
    let repo = PathBuf::from(
        Git::quick(checkout)
            .trimmed(&["rev-parse", "--path-format=absolute", "--show-toplevel"])
            .map_err(|_| "the agent's folder is not in a git repository")?,
    );
    let main = main_checkout(&repo).unwrap_or_else(|| repo.clone());
    let start_sha = Git::quick(&repo)
        .trimmed(&["rev-parse", "--verify", "HEAD"])
        .map_err(|_| "the project has no commit to branch from")?;
    let base = Git::quick(&repo)
        .trimmed(&["symbolic-ref", "--quiet", "--short", "HEAD"])
        .ok();
    let (branch, path) = cut(&repo, name, &start_sha)?;
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
        },
        path.join(inside),
    ))
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

/// Where a saved chat that ran in `cwd`, a worktree of `project`, resumes: back in that
/// worktree while it is there, otherwise in the project.
pub(crate) fn resumed_in(cwd: PathBuf, project: PathBuf) -> (PathBuf, Option<AgentWorktree>) {
    if !cwd.is_dir() {
        return (project, None);
    }
    let worktree = read_back(&cwd);
    (cwd, worktree)
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
    })
}

/// Gives an agent a worktree of its own cut from where its checkout is now,
/// moves it there, and starts it again there if it was running.
pub(crate) async fn for_agent(core: &Arc<Core>, params: ForAgentParams) -> CoreResult<ForAgent> {
    let agent = core
        .workspaces
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
    let message = params.message;
    let (worktree, tree_cwd) = blocking(move || Ok(cut_for(&checkout, &cwd, &message)?)).await?;
    let project = core
        .workspaces
        .view()
        .projects
        .into_iter()
        .find(|project| project.id == agent.project)
        .map_or(agent.cwd, |project| project.path);
    let setup_error = set_up(core, &project, Some(&params.agent_id), &tree_cwd).await;
    core.workspaces.edit_agent(&params.agent_id, |agent| {
        agent.worktree = Some(worktree.clone());
        agent.cwd = tree_cwd.clone();
    })?;
    if core.chats.stop(&params.agent_id) {
        launching::start_agent(core, None, &params.agent_id, false).await?;
    }
    Ok(ForAgent {
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
    let (repo, commit) = (project.to_path_buf(), commit.to_owned());
    let (top, path, fresh) = blocking(move || {
        let top = PathBuf::from(Git::quick(&repo).trimmed(&[
            "rev-parse",
            "--path-format=absolute",
            "--show-toplevel",
        ])?);
        let path = worktrees_folder(&top).join(INTEGRATION);
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
    for hook in hooks {
        let ran = custom_command::run(CommandParams {
            command: hook.command,
            cwd: Some(cwd.to_path_buf()),
            env: HashMap::from([(vars::PROJECT.to_owned(), cwd.to_string_lossy().into_owned())]),
        })
        .await;
        let failed = match ran {
            Ok(result) if result.code == 0 => continue,
            Ok(result) => match result.output.trim() {
                "" => format!("exit {}", result.code),
                output => output.to_owned(),
            },
            Err(error) => error.to_string(),
        };
        return Some(format!("{} failed: {failed}", hook.label));
    }
    None
}
