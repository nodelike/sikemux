//! Projects the core makes rather than finds: a new repository from just a name, or a clone of
//! one by its URL. Both go in the folder the person keeps projects in. A clone becomes a project
//! at once and comes down in the background, its progress in the project's `cloning`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use sikemux_process::sync::Locked;
use sikemux_view::workspace::{Cloning, ProjectInfo};
use tokio::io::AsyncReadExt;
use tokio::task::AbortHandle;

use super::changed::{Topic, What};
use super::connection::blocking;
use super::workspace::AddProjectParams;
use super::{Core, CoreError, CoreResult};
use crate::git::{git_command, Git};

#[derive(Deserialize, Serialize)]
pub struct NewProjectParams {
    pub name: String,
}

#[derive(Deserialize, Serialize)]
pub struct CloneProjectParams {
    pub url: String,
}

/// The clones under way, by project id.
#[derive(Default)]
pub(crate) struct Clones(Mutex<HashMap<String, Running>>);

struct Running {
    shown: Cloning,
    folder: PathBuf,
    task: Option<AbortHandle>,
}

impl Clones {
    pub(crate) fn of(&self, project: &str) -> Option<Cloning> {
        self.0
            .locked()
            .get(project)
            .map(|clone| clone.shown.clone())
    }

    fn set(&self, project: &str, change: impl FnOnce(&mut Cloning)) {
        if let Some(clone) = self.0.locked().get_mut(project) {
            change(&mut clone.shown);
        }
    }

    /// Whether git is still cloning into `folder`, which is no repository to read until it is done.
    pub(crate) fn holds(&self, folder: &Path) -> bool {
        (self.0.locked().values()).any(|clone| clone.folder == folder)
    }

    /// Stops the clone into `project`, answering whether one was there.
    pub(crate) fn stop(&self, project: &str) -> bool {
        let Some(Running { task, .. }) = self.0.locked().remove(project) else {
            return false;
        };
        if let Some(task) = task {
            task.abort();
        }
        true
    }
}

/// A folder name from what the person called the project: `Pinball Stats` is `pinball-stats`.
pub fn folder_name(name: &str) -> String {
    let mut folder = String::new();
    for c in name.trim().chars().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() || c == '.' || c == '_' {
            folder.push(c);
        } else if !folder.ends_with('-') && !folder.is_empty() {
            folder.push('-');
        }
    }
    let folder: String = folder.trim_matches(['-', '.']).chars().take(64).collect();
    match folder.trim_end_matches('-') {
        "" => "project".into(),
        folder => folder.to_owned(),
    }
}

/// The repository's own name in a git address, `app` for `git@host:team/app.git`; none for
/// text that is not one.
pub fn repo_name(url: &str) -> Option<String> {
    let url = url.trim();
    let scp = url
        .split_once(':')
        .is_some_and(|(host, path)| host.contains('@') && !host.contains('/') && !path.is_empty());
    let schemed = ["https://", "http://", "ssh://", "git://", "file://"]
        .iter()
        .any(|scheme| url.starts_with(scheme));
    if !(scp || schemed) || url.contains(char::is_whitespace) {
        return None;
    }
    let last = url.trim_end_matches('/').rsplit(['/', ':']).next()?;
    let name: String = (last.trim_end_matches(".git").chars())
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        .collect();
    let name = name.trim_matches('.');
    (!name.is_empty()).then(|| name.to_owned())
}

fn inside_work_tree(dir: &Path) -> bool {
    Git::quick(dir)
        .trimmed(&["rev-parse", "--is-inside-work-tree"])
        .is_ok_and(|inside| inside == "true")
}

/// The folder the person keeps projects in: their first project root, else the folder most of
/// this host's projects sit in, else the home folder. Never inside a git working tree, where a
/// new repository would land as a stray one in someone else's.
pub(crate) fn projects_folder(core: &Core) -> Option<PathBuf> {
    let roots = (core.preferences.get().project_roots.into_iter())
        .map(|root| sikemux_process::paths::expand_home(&root.path))
        .filter(|root| root.is_dir());
    let mut parents: Vec<PathBuf> = (core.workspaces.project_paths().into_iter())
        .filter_map(|path| path.parent().map(Path::to_path_buf))
        .collect();
    parents.sort();
    let mut groups: Vec<&[PathBuf]> = parents.chunk_by(|a, b| a == b).collect();
    groups.sort_by_key(|same| std::cmp::Reverse(same.len()));
    let beside = groups.iter().map(|same| same[0].clone());
    roots
        .chain(beside)
        .chain(sikemux_process::paths::home())
        .find(|parent| !inside_work_tree(parent))
}

/// Makes a new empty folder for `name` in `parent`, `name-2` and on when it is taken. Making it
/// claims it, so two asks for one name never share a folder.
pub(crate) fn claim_folder(parent: &Path, name: &str) -> Result<PathBuf, String> {
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("could not make {}: {error}", parent.display()))?;
    for n in 1..100 {
        let dir = parent.join(match n {
            1 => name.to_owned(),
            n => format!("{name}-{n}"),
        });
        match std::fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("could not make {}: {error}", dir.display())),
        }
    }
    Err(format!(
        "{} already has too many folders named {name}",
        parent.display()
    ))
}

fn home_of(core: &Core) -> CoreResult<PathBuf> {
    projects_folder(core)
        .ok_or_else(|| CoreError::from("this host has no folder outside a git repository for it"))
}

/// A new repository named for the project, in the projects folder.
pub(crate) async fn create(core: &Arc<Core>, params: NewProjectParams) -> CoreResult<ProjectInfo> {
    let name = params.name.trim().to_owned();
    if name.is_empty() {
        return Err("a new project needs a name".into());
    }
    let parent = home_of(core)?;
    let folder = folder_name(&name);
    let dir = blocking(move || {
        let dir = claim_folder(&parent, &folder)?;
        let branch = Git::quick(&dir)
            .trimmed(&["config", "--get", "init.defaultBranch"])
            .ok()
            .filter(|branch| !branch.is_empty())
            .unwrap_or_else(|| "main".into());
        let made = Git::quick(&dir).trimmed(&["init", "--quiet", "--initial-branch", &branch]);
        match made {
            Ok(_) => Ok(dir),
            Err(error) => {
                let _ = std::fs::remove_dir_all(&dir);
                Err(CoreError::from(format!(
                    "git could not start the repository: {error}"
                )))
            }
        }
    })
    .await?;
    super::workspace::add_project(core, AddProjectParams::named(dir, name)).await
}

/// Starts cloning `url` into a new folder in the projects folder, a project from the start.
pub(crate) async fn clone(core: &Arc<Core>, params: CloneProjectParams) -> CoreResult<ProjectInfo> {
    let url = params.url.trim().to_owned();
    let name = repo_name(&url).ok_or("that does not look like a repository's address")?;
    let parent = home_of(core)?;
    let folder = name.clone();
    let dir = blocking(move || claim_folder(&parent, &folder).map_err(CoreError::from)).await?;
    let project =
        match super::workspace::add_project(core, AddProjectParams::named(dir.clone(), name)).await
        {
            Ok(project) => project,
            Err(error) => {
                let _ = std::fs::remove_dir(&dir);
                return Err(error);
            }
        };
    let cloning = Cloning {
        url: url.clone(),
        phase: "Starting".into(),
        ..Cloning::default()
    };
    core.clones.0.locked().insert(
        project.id.clone(),
        Running {
            shown: cloning,
            folder: project.path.clone(),
            task: None,
        },
    );
    let task = tokio::spawn(run(core.clone(), project.id.clone(), url, dir));
    if let Some(clone) = core.clones.0.locked().get_mut(&project.id) {
        clone.task = Some(task.abort_handle());
    }
    core.changed(What::View);
    Ok(ProjectInfo {
        cloning: core.clones.of(&project.id),
        ..project
    })
}

async fn run(core: Arc<Core>, project: String, url: String, dir: PathBuf) {
    let said = clone_into(&core, &project, &url, &dir).await;
    match said {
        Ok(()) => {
            core.clones.0.locked().remove(&project);
            super::git::follow_projects(&core);
            let folder = dir.to_string_lossy();
            core.changed(What::ReadIn(Topic::Git, Some(&folder), None));
        }
        Err(error) => core.clones.set(&project, |cloning| {
            cloning.failed = Some(error);
        }),
    }
    core.changed(What::View);
}

async fn clone_into(core: &Core, project: &str, url: &str, dir: &Path) -> Result<(), String> {
    let mut command = git_command(dir);
    command.args(["clone", "--progress", "--", url, "."]);
    command.env(
        "GIT_SSH_COMMAND",
        sikemux_process::user_environment::var("GIT_SSH_COMMAND")
            .unwrap_or_else(|| "ssh -o BatchMode=yes -o ConnectTimeout=20".into()),
    );
    let mut child = tokio::process::Command::from(command)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| format!("git did not start: {error}"))?;
    let mut stderr = child.stderr.take().ok_or("git's output was not kept")?;
    let mut said = Vec::new();
    let mut chunk = [0u8; 4096];
    let mut last = String::new();
    loop {
        let read = stderr
            .read(&mut chunk)
            .await
            .map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        said.extend_from_slice(&chunk[..read]);
        let text = String::from_utf8_lossy(&said);
        let mut lines = text
            .split(['\r', '\n'])
            .filter(|line| !line.trim().is_empty());
        if let Some(line) = lines.next_back() {
            if let Some((phase, percent)) = progress(line) {
                let shown = format!("{phase}{percent}");
                if shown != last {
                    last = shown;
                    core.clones.set(project, |cloning| {
                        cloning.phase = phase;
                        cloning.percent = percent;
                    });
                    core.changed(What::View);
                }
            }
        }
        if said.len() > 64 * 1024 {
            said.drain(..said.len() - 4096);
        }
    }
    let status = child.wait().await.map_err(|error| error.to_string())?;
    if status.success() {
        return Ok(());
    }
    let text = String::from_utf8_lossy(&said);
    let why = (text.split(['\r', '\n']).map(str::trim))
        .rfind(|line| line.starts_with("fatal:") || line.starts_with("ERROR"))
        .or_else(|| text.lines().map(str::trim).rfind(|line| !line.is_empty()))
        .unwrap_or("git could not clone it");
    Err(why.trim_start_matches("fatal:").trim().to_owned())
}

/// The phase and how far the whole clone is from one of git's progress lines. Receiving the
/// objects is most of a clone, so it fills the middle of the bar.
fn progress(line: &str) -> Option<(String, u8)> {
    let line = line.trim().trim_start_matches("remote:").trim();
    let (phase, rest) = line.split_once(':')?;
    let percent: u8 = rest.trim().split('%').next()?.trim().parse().ok()?;
    let (from, span) = match phase {
        "Enumerating objects" | "Counting objects" | "Compressing objects" => (0, 10),
        "Receiving objects" => (10, 75),
        "Resolving deltas" => (85, 10),
        "Updating files" => (95, 5),
        _ => return None,
    };
    let whole = from + u32::from(percent.min(100)) * span / 100;
    Some((phase.to_owned(), whole as u8))
}

/// Removes the folder of a clone that was stopped or failed, which holds nothing to keep.
pub(crate) fn forget(core: &Core, project: &str, path: &Path) {
    if core.clones.stop(project) {
        let _ = std::fs::remove_dir_all(path);
    }
}
