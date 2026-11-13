//! What a folder's checkout gives everything Sikemux starts in it: its name, its ports and the
//! project's environment, and what the person and agents are told of them.

use crate::server::repo_state::RepoParts;
use crate::server::workspace_state::WorkspaceParts;
pub use sikemux_calls::ports::{CheckoutAbout, CheckoutParams, CheckoutPort};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::blocks::Held;
use super::ListeningPort;
use crate::server::connection::blocking;
use crate::server::worktree::AgentWorktree;
use crate::server::{Core, CoreResult};
use crate::tasks::checkout::{self as values, Place, MAIN};
use crate::tasks::config::{self, ConfigLoad, ProjectConfig, CONFIG_FILE};

/// The checkout a folder lies in, as its processes see it.
pub struct Here {
    pub name: String,
    pub root: PathBuf,
    pub first_port: Option<u16>,
    /// The project's `sikemux.json` there, when it reads.
    pub config: Option<ProjectConfig>,
    /// The person trusts that file, so its `env` is given.
    pub trusted: bool,
}

impl Here {
    pub fn place(&self) -> Place<'_> {
        Place {
            name: &self.name,
            root: &self.root,
            first_port: self.first_port,
        }
    }

    pub fn ports(&self) -> Vec<(String, u16)> {
        (self.config.as_ref()).map_or_else(Vec::new, |config| values::ports(config, &self.place()))
    }

    /// Everything a process started here is given.
    pub fn environment(&self) -> Vec<(String, String)> {
        let config = self.config.as_ref().filter(|_| self.trusted);
        let mut env = values::environment(config, &self.place());
        if config.is_none() {
            env.extend(
                self.ports()
                    .into_iter()
                    .map(|(name, port)| (values::port_variable(&name), port.to_string())),
            );
        }
        env
    }

    /// What the person is shown of what processes here get: the checkout's name and the
    /// project's own `env`, as the ports and folder are shown apart.
    pub fn shown_environment(&self) -> Vec<(String, String)> {
        let name = (values::CHECKOUT_VAR.to_owned(), self.name.clone());
        let config = self.config.as_ref().filter(|_| self.trusted);
        let ports = self.ports();
        let own = config.into_iter().flat_map(|config| {
            (config.env.iter())
                .map(|(key, value)| (key.clone(), values::fill(value, &self.place(), &ports)))
        });
        std::iter::once(name).chain(own).collect()
    }

    /// What a process `task_id` started here adds: `PORT` when it has one, and its own `env`.
    pub fn task_environment(&self, task_id: &str) -> Vec<(String, String)> {
        let Some(config) = &self.config else {
            return Vec::new();
        };
        let Some(task) = config.tasks.iter().find(|task| task.id == task_id) else {
            return Vec::new();
        };
        let ports = self.ports();
        let port = task
            .port
            .and_then(|_| ports.iter().find(|(name, _)| *name == task.id));
        let filled = task
            .env
            .iter()
            .map(|(key, value)| (key.clone(), values::fill(value, &self.place(), &ports)));
        port.map(|(_, port)| ("PORT".to_owned(), port.to_string()))
            .into_iter()
            .chain(filled)
            .collect()
    }

    /// The port `task_id` listens on here, when the project gave it one.
    pub fn task_port(&self, task_id: &str) -> Option<u16> {
        let config = self.config.as_ref()?;
        config.tasks.iter().find(|task| task.id == task_id)?.port?;
        (self.ports().into_iter()).find_map(|(name, port)| (name == task_id).then_some(port))
    }

    /// `text` with this checkout's values in place of `{port.<name>}`, `{checkout}` and `{root}`.
    pub fn fill(&self, text: &str) -> String {
        values::fill(text, &self.place(), &self.ports())
    }
}

/// The checkout `cwd` lies in: one the core cut, by the ports it gave it, or a project's own
/// folder, named `main`. Reads the disk.
pub fn here(core: &Core, cwd: &Path) -> Option<Here> {
    let cwd = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_owned());
    let held = core.ports().blocks.of(&cwd).or_else(|| {
        let (repo, path, branch) = cut_by_core(&cwd)?;
        reserve(core, &repo, &path, &branch).ok()
    });
    let (name, root, first_port) = match held {
        Some(held) => (held.name, held.path, Some(held.first_port)),
        None => {
            let projects = core.workspaces().project_paths();
            let project = projects
                .iter()
                .map(|project| std::fs::canonicalize(project).unwrap_or_else(|_| project.clone()))
                .filter(|project| cwd.starts_with(project))
                .max_by_key(|project| project.as_os_str().len())?;
            (MAIN.to_owned(), project, None)
        }
    };
    let folder = cwd
        .ancestors()
        .take_while(|folder| folder.starts_with(&root))
        .find(|folder| folder.join(CONFIG_FILE).is_file());
    let load = folder.map(config::load);
    let trusted = load
        .as_ref()
        .is_some_and(|load| (core.repo_above().tasks_config_needs)(core, load).is_none());
    let config = match load {
        Some(ConfigLoad::Valid { config, .. }) => Some(*config),
        _ => None,
    };
    if first_port.is_none() && config.is_none() {
        return None;
    }
    Some(Here {
        name,
        root,
        first_port,
        config,
        trusted,
    })
}

/// The project's own copy of `file`, a file in a checkout the core cut.
pub fn projects_own(core: &Core, file: &Path) -> Option<PathBuf> {
    let file = std::fs::canonicalize(file).ok()?;
    let held = core.ports().blocks.of(&file)?;
    Some(held.repo.join(file.strip_prefix(&held.path).ok()?))
}

/// `url` with the values of the checkout `dir` lies in filled in.
pub fn filled(core: &Core, dir: &Path, url: &str) -> String {
    here(core, dir).map_or_else(|| url.to_owned(), |here| here.fill(url))
}

/// What everything started in `cwd` is given, put under what `env` holds already.
pub fn give(core: &Core, cwd: &Path, env: &mut HashMap<String, String>) {
    for (key, value) in here(core, cwd)
        .map(|here| here.environment())
        .unwrap_or_default()
    {
        env.entry(key).or_insert(value);
    }
}

fn reserve(core: &Core, repo: &Path, path: &Path, branch: &str) -> Result<Held, String> {
    core.ports()
        .blocks
        .reserve(repo, path, branch, super::in_use, |path| {
            (core.workspaces().view().agents.iter()).any(|agent| {
                agent
                    .worktree
                    .as_ref()
                    .is_some_and(|tree| tree.path == path)
            })
        })
}

/// The main checkout, folder and branch of the checkout the core cut that `cwd` lies in: a
/// folder of `<repo>.worktrees` other than the core's own and reviewers' read-only ones.
fn cut_by_core(cwd: &Path) -> Option<(PathBuf, PathBuf, String)> {
    cwd.ancestors().find_map(|path| {
        let folder = path.parent()?;
        let repo_name = folder.file_name()?.to_str()?.strip_suffix(".worktrees")?;
        let name = path.file_name()?.to_str()?;
        if name.starts_with('.') || name.starts_with("review-") {
            return None;
        }
        let repo = folder.with_file_name(repo_name);
        let git = std::fs::read_to_string(path.join(".git")).ok()?;
        let own = PathBuf::from(git.strip_prefix("gitdir:")?.trim());
        if !repo.join(".git").is_dir() {
            return None;
        }
        let head = std::fs::read_to_string(own.join("HEAD")).unwrap_or_default();
        let branch = head
            .trim()
            .strip_prefix("ref: refs/heads/")
            .map_or_else(|| name.to_owned(), str::to_owned);
        Some((repo, path.to_owned(), branch))
    })
}

/// Gives the agent checkout `tree` its name and ports, kept from before when it had them.
pub fn claim(core: &Core, tree: &mut AgentWorktree) {
    let claimed = reserve(core, &tree.repo, &tree.path, &tree.branch);
    match claimed {
        Ok(held) => {
            tree.name = held.name;
            tree.first_port = Some(held.first_port);
        }
        Err(error) => eprintln!(
            "sikemux core: {} has no ports of its own: {error}",
            tree.path.display()
        ),
    }
}

/// Runs the project's `worktree.onRemove` steps in the checkout at `tree` while the person trusts
/// its `sikemux.json`, then frees its name and ports. Why a step failed, when one did.
pub async fn removing(core: &Arc<Core>, tree: &Path) -> Option<String> {
    let found = {
        let (core, tree) = (core.clone(), tree.to_owned());
        blocking(move || {
            Ok(here(&core, &tree).map(|here| (here.environment(), here.config, here.trusted)))
        })
        .await
        .ok()
        .flatten()
    };
    let failed = match found {
        Some((env, Some(config), true)) => {
            let hooks = config
                .worktree
                .map(|worktree| worktree.on_remove)
                .unwrap_or_default();
            crate::server::worktree::agent::run_hooks(hooks, tree, env)
                .await
                .map(|failed| format!("{} failed: {}", failed.label, failed.output))
        }
        _ => None,
    };
    core.ports().blocks.free(tree);
    if let Some(failed) = &failed {
        eprintln!("sikemux core: removing {}: {failed}", tree.display());
    }
    failed
}

pub fn about(here: &Here, listening: &[ListeningPort]) -> CheckoutAbout {
    let ports = here
        .ports()
        .into_iter()
        .map(|(name, port)| CheckoutPort {
            process: (listening.iter())
                .find(|listener| listener.port == port)
                .map(|listener| listener.process.clone()),
            name,
            port,
        })
        .collect();
    let main_ports = here.config.as_ref().map_or_else(Vec::new, |config| {
        (config.port_names().into_iter())
            .map(|(name, port)| (name.to_owned(), port))
            .collect()
    });
    CheckoutAbout {
        name: here.name.clone(),
        path: here.root.clone(),
        first_port: here.first_port,
        ports,
        main_ports,
        env: here.shown_environment(),
    }
}

/// The checkout `cwd` lies in, as the person is shown it; none outside every checkout.
pub async fn checkout(
    core: &Arc<Core>,
    params: CheckoutParams,
) -> CoreResult<Option<CheckoutAbout>> {
    let core = core.clone();
    blocking(move || {
        let Some(here) = here(&core, &params.cwd) else {
            return Ok(None);
        };
        Ok(Some(about(&here, &super::listening_now(&core))))
    })
    .await
}
