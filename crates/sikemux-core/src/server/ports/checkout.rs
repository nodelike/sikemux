//! What a folder's checkout gives everything Sikemux starts in it: its name, its ports and the
//! project's environment, and what the person and agents are told of them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::blocks::{Held, BLOCK};
use super::ListeningPort;
use crate::server::connection::blocking;
use crate::server::parts::Parts;
use crate::server::tasks;
use crate::server::worktree::AgentWorktree;
use crate::server::{Core, CoreResult};
use crate::tasks::checkout::{self as values, Place, MAIN};
use crate::tasks::config::{self, ConfigLoad, ProjectConfig, CONFIG_FILE};

/// The checkout a folder lies in, as its processes see it.
pub(crate) struct Here {
    pub name: String,
    pub root: PathBuf,
    pub first_port: Option<u16>,
    /// The project's `sikemux.json` there, when it reads.
    pub config: Option<ProjectConfig>,
    /// The person trusts that file, so its `env` is given.
    pub trusted: bool,
}

impl Here {
    pub(crate) fn place(&self) -> Place<'_> {
        Place {
            name: &self.name,
            root: &self.root,
            first_port: self.first_port,
        }
    }

    pub(crate) fn ports(&self) -> Vec<(String, u16)> {
        (self.config.as_ref()).map_or_else(Vec::new, |config| values::ports(config, &self.place()))
    }

    /// Everything a process started here is given.
    pub(crate) fn environment(&self) -> Vec<(String, String)> {
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

    /// What a process `task_id` started here adds: `PORT` when it has one, and its own `env`.
    pub(crate) fn task_environment(&self, task_id: &str) -> Vec<(String, String)> {
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
    pub(crate) fn task_port(&self, task_id: &str) -> Option<u16> {
        let config = self.config.as_ref()?;
        config.tasks.iter().find(|task| task.id == task_id)?.port?;
        (self.ports().into_iter()).find_map(|(name, port)| (name == task_id).then_some(port))
    }

    /// `text` with this checkout's values in place of `{port.<name>}`, `{checkout}` and `{root}`.
    pub(crate) fn fill(&self, text: &str) -> String {
        values::fill(text, &self.place(), &self.ports())
    }
}

/// The checkout `cwd` lies in: one the core cut, by the ports it gave it, or a project's own
/// folder, named `main`. Reads the disk.
pub(crate) fn here(core: &Core, cwd: &Path) -> Option<Here> {
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
    let load = cwd
        .ancestors()
        .take_while(|folder| folder.starts_with(&root))
        .find(|folder| folder.join(CONFIG_FILE).is_file())
        .map(config::load);
    let trusted = load
        .as_ref()
        .is_some_and(|load| tasks::config_needs(core, load).is_none());
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

/// `url` with the values of the checkout `dir` lies in filled in.
pub(crate) fn filled(core: &Core, dir: &Path, url: &str) -> String {
    here(core, dir).map_or_else(|| url.to_owned(), |here| here.fill(url))
}

/// What everything started in `cwd` is given, put under what `env` holds already.
pub(crate) fn give(core: &Core, cwd: &Path, env: &mut HashMap<String, String>) {
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
pub(crate) fn claim(core: &Core, tree: &mut AgentWorktree) {
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
pub(crate) async fn removing(core: &Arc<Core>, tree: &Path) -> Option<String> {
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

/// A port of a checkout, and what listens on it now.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutPort {
    pub name: String,
    pub port: u16,
    /// The program that listens, when one does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process: Option<String>,
}

/// A checkout's name, ports and environment, as the person and agents are shown them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutAbout {
    pub name: String,
    pub path: PathBuf,
    /// The first of its own ten, none in the project's own folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_port: Option<u16>,
    pub ports: Vec<CheckoutPort>,
    /// Each name with the port the project's own folder keeps.
    pub main_ports: Vec<(String, u16)>,
    pub env: Vec<(String, String)>,
}

impl CheckoutAbout {
    /// Its ports no name takes, as a first and last.
    pub fn unnamed(&self) -> Option<(u16, u16)> {
        let first = self.first_port?;
        let next = first + self.ports.len() as u16;
        (next < first + BLOCK).then_some((next, first + BLOCK - 1))
    }
}

pub(crate) fn about(here: &Here, listening: &[ListeningPort]) -> CheckoutAbout {
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
        env: here.environment(),
    }
}

#[derive(Serialize, Deserialize)]
pub struct CheckoutParams {
    pub cwd: PathBuf,
}

/// The checkout `cwd` lies in, as the person is shown it; none outside every checkout.
pub(crate) async fn checkout(
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
