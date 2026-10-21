//! Each piece of work's own checkout: a git worktree on a branch of its own,
//! in a folder beside the project's repository so it never shows up in the
//! project's own tree or status. Chat agents, Auto's cards and workers for
//! another host's agents all get theirs here.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sikemux_process::vars;

use super::board::git::{branch_exists, git, git_in, inside, slug, worktrees_folder, SLOW};
use super::connection::blocking;
use super::custom_command::{self, CommandParams};
use super::{launching, tasks, Core, CoreResult};
use crate::tasks::config::{self, ConfigLoad};
use crate::tasks::detect::PERSONAL_TASKS;

const BRANCH_PREFIX: &str = "sikemux/";
const NAME_TRIES: usize = 50;

/// Without a `.worktreeinclude`, the files a project usually keeps out of git
/// but needs to run.
const DEFAULT_INCLUDE: [&str; 6] = [
    ".env",
    ".env.*",
    "*/.env",
    "*/.env.*",
    ".sikemux/.gitignore",
    PERSONAL_TASKS,
];
const MAX_INCLUDED: usize = 50;

/// Cuts a worktree of `repo` at `start` on a new branch named for `name`.
/// The branch and the folder it is in.
pub(crate) fn cut(repo: &Path, name: &str, start: &str) -> Result<(String, PathBuf), String> {
    let folder = worktrees_folder(repo);
    let base = slug(name);
    let (branch, path) = (1..=NAME_TRIES)
        .map(|index| {
            let slug = if index == 1 {
                base.clone()
            } else {
                format!("{base}-{index}")
            };
            (format!("{BRANCH_PREFIX}{slug}"), folder.join(&slug))
        })
        .find(|(branch, path)| !path.exists() && !branch_exists(repo, branch))
        .ok_or_else(|| format!("every worktree name for {base} is taken"))?;
    std::fs::create_dir_all(&folder)
        .map_err(|error| format!("could not make {}: {error}", folder.display()))?;
    let target = path.to_string_lossy().into_owned();
    git_in(
        repo,
        &[
            "worktree",
            "add",
            "--no-track",
            "-b",
            &branch,
            &target,
            start,
        ],
        SLOW,
    )?;
    Ok((branch, path))
}

/// Takes away a worktree and its branch.
pub(crate) fn remove(repo: &Path, path: &Path, branch: &str) {
    let target = path.to_string_lossy().into_owned();
    if let Err(error) = git(repo, &["worktree", "remove", "--force", &target]) {
        eprintln!("sikemux core: the worktree {target} is left: {error}");
    }
    let _ = git(repo, &["branch", "-D", branch]);
}

/// Copies what a fresh checkout lacks but needs to run, such as `.env`, from
/// `main` into the worktree at `tree`: what `.worktreeinclude` lists, the
/// file Claude Code and Codex read, or else [`DEFAULT_INCLUDE`].
pub(crate) fn copy_included(main: &Path, tree: &Path) {
    let listed = std::fs::read_to_string(main.join(".worktreeinclude")).ok();
    let patterns: Vec<&str> = match &listed {
        Some(listed) => listed
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .collect(),
        None => DEFAULT_INCLUDE.to_vec(),
    };
    let mut found: Vec<PathBuf> = patterns
        .iter()
        .flat_map(|pattern| expand(main, pattern))
        .collect();
    found.sort();
    found.dedup();
    for relative in found.into_iter().take(MAX_INCLUDED) {
        let (from, to) = (main.join(&relative), tree.join(&relative));
        if !from.is_file() || to.symlink_metadata().is_ok() {
            continue;
        }
        if let Some(parent) = to.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(error) = std::fs::copy(&from, &to) {
            eprintln!(
                "sikemux core: {} was not copied into the worktree: {error}",
                relative.display()
            );
        }
    }
}

/// The paths below `root` that `pattern` names, where `*` matches within one
/// part of a path.
fn expand(root: &Path, pattern: &str) -> Vec<PathBuf> {
    let mut paths = vec![PathBuf::new()];
    let parts: Vec<&str> = pattern.split('/').filter(|part| !part.is_empty()).collect();
    for (index, part) in parts.iter().enumerate() {
        if *part == ".." {
            return Vec::new();
        }
        let last = index + 1 == parts.len();
        let mut next = Vec::new();
        for base in &paths {
            if !part.contains('*') {
                next.push(base.join(part));
                continue;
            }
            let Ok(entries) = std::fs::read_dir(root.join(base)) else {
                continue;
            };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
                if matches_part(part, &name) && (last || is_dir) {
                    next.push(base.join(name));
                }
            }
        }
        paths = next;
    }
    paths
}

fn matches_part(pattern: &str, name: &str) -> bool {
    let mut pieces = pattern.split('*');
    let first = pieces.next().unwrap_or_default();
    let Some(mut rest) = name.strip_prefix(first) else {
        return false;
    };
    let pieces: Vec<&str> = pieces.collect();
    for (index, piece) in pieces.iter().enumerate() {
        if index + 1 == pieces.len() {
            return rest.len() >= piece.len() && rest.ends_with(piece);
        }
        match rest.find(piece) {
            Some(at) => rest = &rest[at + piece.len()..],
            None => return false,
        }
    }
    rest.is_empty()
}

/// The checkout an agent was given, as its workspace record keeps it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentWorktree {
    /// The main checkout, which git commands about the worktree run against.
    pub repo: PathBuf,
    pub path: PathBuf,
    pub branch: String,
    /// The branch the project was on when the worktree was cut, if any.
    pub base: Option<String>,
    pub start_sha: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ForAgentParams {
    agent_id: String,
    /// What the agent was asked first, which names the branch.
    message: String,
    /// The repository to use when the agent's folder holds several.
    repo: Option<PathBuf>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ForAgent {
    worktree: AgentWorktree,
    cwd: PathBuf,
    /// Why the project's worktree setup did not all finish, when it did not.
    setup_error: Option<String>,
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
        git(
            checkout,
            &["rev-parse", "--path-format=absolute", "--show-toplevel"],
        )
        .map_err(|_| "the agent's folder is not in a git repository")?,
    );
    let main = PathBuf::from(git(
        &repo,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?)
    .parent()
    .map(Path::to_path_buf)
    .unwrap_or_else(|| repo.clone());
    let start_sha = git(&repo, &["rev-parse", "--verify", "HEAD"])
        .map_err(|_| "the project has no commit to branch from")?;
    let base = git(&repo, &["symbolic-ref", "--quiet", "--short", "HEAD"]).ok();
    let (branch, path) = cut(&repo, name, &start_sha)?;
    copy_included(&main, &path);
    link_installed(&main, &path);
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

fn read_back(cwd: &Path) -> Option<AgentWorktree> {
    let path = PathBuf::from(
        git(
            cwd,
            &["rev-parse", "--path-format=absolute", "--show-toplevel"],
        )
        .ok()?,
    );
    let main = PathBuf::from(
        git(
            &path,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )
        .ok()?,
    )
    .parent()?
    .to_path_buf();
    if !path.starts_with(worktrees_folder(&main)) {
        return None;
    }
    let branch = git(&path, &["symbolic-ref", "--quiet", "--short", "HEAD"]).ok()?;
    let base = git(&main, &["symbolic-ref", "--quiet", "--short", "HEAD"]).ok();
    let start_sha = git(&main, &["merge-base", "HEAD", &branch]).ok()?;
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
    let mut patch = Map::new();
    patch.insert("worktree".into(), serde_json::to_value(&worktree)?);
    patch.insert(
        "cwd".into(),
        Value::String(tree_cwd.to_string_lossy().into_owned()),
    );
    core.workspaces.update_agent(&params.agent_id, patch)?;
    core.view_changed();
    if core.chats.stop(&params.agent_id) {
        launching::start_agent(core, None, &params.agent_id).await?;
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
        let top = PathBuf::from(git(
            &repo,
            &["rev-parse", "--path-format=absolute", "--show-toplevel"],
        )?);
        let path = worktrees_folder(&top).join(INTEGRATION);
        let fresh = !path.join(".git").exists();
        let target = path.to_string_lossy().into_owned();
        if fresh {
            let _ = git(&top, &["worktree", "prune"]);
            git_in(
                &top,
                &["worktree", "add", "--detach", "--force", &target, &commit],
                SLOW,
            )?;
            copy_included(&top, &path);
            link_installed(&top, &path);
        } else {
            git_in(
                &path,
                &["checkout", "--detach", "--force", "--quiet", &commit],
                SLOW,
            )?;
            git(&path, &["clean", "-fdq"])?;
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

/// Links each `node_modules` beside a `package.json` at the top of `main`, or
/// one folder down, into `tree`, so what runs there finds what is installed.
fn link_installed(main: &Path, tree: &Path) {
    let mut folders = vec![PathBuf::new()];
    if let Ok(entries) = std::fs::read_dir(main) {
        folders.extend(
            entries
                .flatten()
                .filter(|entry| entry.path().join("package.json").is_file())
                .map(|entry| PathBuf::from(entry.file_name())),
        );
    }
    for folder in folders {
        let installed = main.join(&folder).join("node_modules");
        let link = tree.join(&folder).join("node_modules");
        if installed.is_dir()
            && link.symlink_metadata().is_err()
            && link.parent().is_some_and(Path::is_dir)
        {
            let _ = std::os::unix::fs::symlink(&installed, &link);
        }
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_worktree_goes_beside_the_repository_under_a_name_not_yet_taken() {
        let parent = tempfile::tempdir().unwrap();
        let repo = parent.path().join("app");
        std::fs::create_dir(&repo).unwrap();
        for args in [
            &["init", "-q"][..],
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "base",
            ],
            &["branch", "sikemux/fix-the-flaky-pty-test"],
        ] {
            git(&repo, args).unwrap();
        }
        let (branch, path) = cut(&repo, "Fix the flaky PTY test", "HEAD").unwrap();
        assert_eq!(branch, "sikemux/fix-the-flaky-pty-test-2");
        assert_eq!(
            path,
            parent.path().join("app.worktrees/fix-the-flaky-pty-test-2")
        );
        assert!(path.join(".git").exists());
    }
}
