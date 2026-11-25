use super::*;

impl Workspaces {
    pub(super) fn add_project(
        &self,
        path: PathBuf,
        name: Option<String>,
        id: Option<String>,
    ) -> CoreResult<ProjectInfo> {
        let project = self.change(|saved| {
            if let Some(project) = saved.projects.iter().find(|project| project.path == path) {
                return Ok(project.clone());
            }
            if saved.projects.len() >= MAX_PROJECTS {
                return Err("this host already keeps as many projects as it can".into());
            }
            let id = id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            if saved.project(&id).is_some() {
                return Err("a project with that id is already on this host".into());
            }
            let name = name
                .map(|name| {
                    name.trim()
                        .chars()
                        .take(MAX_TITLE_CHARS)
                        .collect::<String>()
                })
                .filter(|name| !name.is_empty())
                .or_else(|| {
                    path.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                })
                .unwrap_or_else(|| path.display().to_string());
            let project = ProjectInfo {
                id,
                name,
                path,
                pinned: false,
                added_at: epoch_ms(),
                space: None,
                space_parent: None,
                space_followed: false,
                code_hosts: Default::default(),
                identity: None,
                copies: Vec::new(),
                git: None,
                launcher: None,
                code_host: None,
                worktree_default: false,
                saved_chats: BTreeMap::new(),
                chief: None,
                chief_count: 0,
                chief_home: None,
                agents: 0,
                standing: Default::default(),
                calls: 0,
                cloning: None,
                shared: None,
            };
            check_project(&project, saved)?;
            saved.projects.push(project.clone());
            Ok(project)
        })?;
        self.recent_stale();
        Ok(project)
    }

    /// Makes `agent_id` the project's chief, its `count`th.
    pub fn set_chief(&self, project_id: &str, agent_id: &str, count: u32) -> CoreResult<()> {
        self.change(|saved| {
            let project = (saved.projects.iter_mut())
                .find(|project| project.id == project_id)
                .ok_or_else(|| CoreError::from("that project is not on this host"))?;
            project.chief = Some(agent_id.to_owned());
            project.chief_count = count;
            Ok(())
        })
    }

    /// The project no longer has a chief of its own, because another host's outranks it. Its
    /// count stays, so the next chief made here comes after it.
    pub fn stand_down(&self, project_id: &str) -> CoreResult<()> {
        self.change(|saved| {
            if let Some(project) =
                (saved.projects.iter_mut()).find(|project| project.id == project_id)
            {
                project.chief = None;
            }
            Ok(())
        })
    }

    /// The project `cwd` is in, added when it is in none, for an agent that
    /// started there.
    pub fn project_for(&self, cwd: &Path) -> CoreResult<String> {
        let path = PathBuf::from(self.project_of(cwd));
        Ok(self.add_project(path, None, None)?.id)
    }

    /// Stops offering a project, and forgets the agents in it, answering its id.
    pub(super) fn remove_project(&self, path: &Path) -> CoreResult<String> {
        let id = self.change(|saved| {
            let id = saved
                .projects
                .iter()
                .find(|project| project.path == path)
                .map(|project| project.id.clone())
                .ok_or_else(|| CoreError::from("that folder is not a project on this host"))?;
            saved.projects.retain(|project| project.id != id);
            saved.agents.retain(|kept| kept.agent.project != id);
            Ok(id)
        })?;
        self.recent_stale();
        Ok(id)
    }

    pub(super) fn update_project(
        &self,
        id: &str,
        patch: Map<String, Value>,
    ) -> CoreResult<ProjectInfo> {
        self.change(|saved| {
            let current = (saved.projects.iter())
                .find(|project| project.id == id)
                .ok_or_else(|| CoreError::from("that project is not on this host"))?;
            let project = patched(
                current,
                patch,
                &[
                    "id",
                    "path",
                    "identity",
                    "copies",
                    "chief",
                    "agents",
                    "standing",
                    "calls",
                    "spaceParent",
                    "spaceFollowed",
                ],
            )?;
            check_project(&project, saved)?;
            if let Some(kept) = saved.projects.iter_mut().find(|known| known.id == id) {
                *kept = project.clone();
            }
            Ok(project)
        })
    }

    /// Keeps what each project is known by.
    pub fn set_identities(&self, identities: Vec<(PathBuf, Option<String>)>) {
        let changed = |saved: &Saved| {
            identities.iter().any(|(path, identity)| {
                saved
                    .projects
                    .iter()
                    .any(|project| &project.path == path && &project.identity != identity)
            })
        };
        if !changed(&self.published.locked().saved) {
            return;
        }
        let _ = self.change(|saved| {
            for project in &mut saved.projects {
                if let Some((_, identity)) =
                    identities.iter().find(|(path, _)| *path == project.path)
                {
                    project.identity = identity.clone();
                }
            }
            Ok(())
        });
    }

    /// When each project was last worked in, by id: its newest saved chat or agent.
    pub fn last_active(&self) -> HashMap<String, u64> {
        let published = self.published.locked();
        let saved = &published.saved;
        let mut last: HashMap<String, u64> = HashMap::new();
        let mut note = |project: &str, at: u64| {
            let kept = last.entry(project.to_owned()).or_default();
            *kept = (*kept).max(at);
        };
        for chat in &published.recent {
            if let Some(project) = saved.projects.iter().find(|p| p.path == chat.project) {
                note(&project.id, chat.active_at);
            }
        }
        for kept in &saved.agents {
            note(&kept.agent.project, kept.agent.created_at);
        }
        last
    }

    /// Lists the projects in the order of `ids`, leaving any it misses after them.
    pub(super) fn order_projects(&self, ids: &[String]) -> CoreResult<()> {
        self.change(|saved| {
            saved.projects.sort_by_key(|project| {
                ids.iter()
                    .position(|id| *id == project.id)
                    .unwrap_or(usize::MAX)
            });
            Ok(())
        })
    }

    pub(super) fn move_project(&self, id: &str, index: usize) -> CoreResult<()> {
        self.change(|saved| {
            let from = saved
                .projects
                .iter()
                .position(|project| project.id == id)
                .ok_or_else(|| CoreError::from("that project is not on this host"))?;
            let project = saved.projects.remove(from);
            let index = index.min(saved.projects.len());
            saved.projects.insert(index, project);
            Ok(())
        })
    }

    /// Replaces the spaces; a project in one that is gone has none of its own.
    pub(super) fn set_spaces(&self, spaces: Vec<SpaceInfo>) -> CoreResult<()> {
        let oversized = spaces.iter().any(|space| {
            too_long(&space.id, MAX_FIELD_CHARS)
                || too_long(&space.name, MAX_TITLE_CHARS)
                || too_long(&space.icon, MAX_FIELD_CHARS)
        });
        if spaces.len() > MAX_SPACES || oversized {
            return Err("more spaces than the core keeps".into());
        }
        self.change(|saved| {
            for project in &mut saved.projects {
                let gone = project
                    .space
                    .as_ref()
                    .is_some_and(|space| !spaces.iter().any(|kept| &kept.id == space));
                if gone {
                    project.space = None;
                }
            }
            saved.spaces = spaces;
            Ok(())
        })
    }

    /// Keeps the mode agents start in, and says whether it changed.
    pub(super) fn set_permission_mode(&self, mode: PermissionMode) -> CoreResult<bool> {
        self.change(|saved| {
            let changed = saved.permission_mode != Some(mode);
            saved.permission_mode = Some(mode);
            Ok(changed)
        })
    }

    pub fn project_names(&self) -> HashMap<String, String> {
        let published = self.published.locked();
        (published.saved.projects.iter())
            .flat_map(|project| {
                [
                    (project.id.clone(), project.name.clone()),
                    (
                        project.path.to_string_lossy().into_owned(),
                        project.name.clone(),
                    ),
                ]
            })
            .collect()
    }

    pub fn project_info(&self, id: &str) -> Option<ProjectInfo> {
        self.published.locked().saved.project(id).cloned()
    }

    pub fn project_path(&self, id: &str) -> CoreResult<PathBuf> {
        self.published
            .locked()
            .saved
            .project(id)
            .map(|project| project.path.clone())
            .ok_or_else(|| CoreError::from("that project is not on this host"))
    }

    pub fn chiefs(&self) -> HashSet<String> {
        (self.published.locked().saved.projects.iter())
            .filter_map(|project| project.chief.clone())
            .collect()
    }

    pub fn is_chief(&self, agent_id: &str) -> bool {
        (self.published.locked().saved.projects.iter())
            .any(|project| project.chief.as_deref() == Some(agent_id))
    }

    /// This host's projects, as kept.
    pub fn projects(&self) -> Vec<ProjectInfo> {
        self.published.locked().saved.projects.clone()
    }

    pub fn project_paths(&self) -> Vec<PathBuf> {
        self.published
            .locked()
            .saved
            .projects
            .iter()
            .map(|project| project.path.clone())
            .collect()
    }

    /// The project an agent working in `cwd` belongs to, as [`project_of`] finds it among
    /// this host's projects.
    pub fn project_of(&self, cwd: &Path) -> String {
        project_of(cwd, &self.project_paths())
    }

    /// The project the checkout at `top` belongs to, and whether it is that
    /// project's own checkout, whose branch is the project's, rather than an
    /// agent's checkout of its own.
    pub fn project_checkout(&self, top: &Path) -> (String, bool) {
        let project = self.project_of(top);
        let own = Path::new(&project).starts_with(top);
        (project, own)
    }
}

/// The project an agent working in `cwd` belongs to: the app's project that
/// holds it, else `cwd` itself. A folder in a git worktree counts as the same
/// folder in the main checkout.
pub fn project_of(cwd: &Path, projects: &[PathBuf]) -> String {
    let canonical =
        |path: &Path| sikemux_process::files::real_path(path).unwrap_or_else(|_| path.to_owned());
    let cwd = canonical(cwd);
    let path = in_main_checkout(&cwd).unwrap_or(cwd);
    projects
        .iter()
        .map(|project| canonical(project))
        .filter(|project| path.starts_with(project))
        .max_by_key(|project| project.as_os_str().len())
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

/// The folder in the main checkout matching `cwd` in a linked git worktree.
pub(super) fn in_main_checkout(cwd: &Path) -> Option<PathBuf> {
    let root = sikemux_process::files::repo_root(cwd)?;
    let main = crate::git::main_checkout(root)?;
    let inside = cwd.strip_prefix(root).ok()?;
    Some(if inside.as_os_str().is_empty() {
        main
    } else {
        main.join(inside)
    })
}

pub(super) fn check_project(project: &ProjectInfo, saved: &Saved) -> CoreResult<()> {
    if too_long(&project.name, MAX_TITLE_CHARS) || too_long(&project.id, MAX_FIELD_CHARS) {
        return Err("the project has a field longer than the core keeps".into());
    }
    if !project.path.is_absolute() {
        return Err(format!("project {} has no absolute path", project.name).into());
    }
    if let Some(space) = &project.space {
        if !saved.spaces.iter().any(|known| &known.id == space) {
            return Err("that space is not on this host".into());
        }
    }
    Ok(())
}

/// The project among `projects` that `folder` lies in, a worktree of one counting as in it.
pub(super) fn home_of(folder: &Path, projects: &[PathBuf]) -> Option<PathBuf> {
    let home = PathBuf::from(project_of(folder, projects));
    projects
        .iter()
        .find(|project| {
            sikemux_process::files::real_path(project)
                .as_deref()
                .unwrap_or(project)
                == home
        })
        .cloned()
}

pub trait AddProjectParamsExt: Sized {
    fn at(path: PathBuf) -> Self;

    fn named(path: PathBuf, name: String) -> Self;
}

impl AddProjectParamsExt for AddProjectParams {
    fn at(path: PathBuf) -> Self {
        Self {
            path,
            name: None,
            id: None,
        }
    }

    fn named(path: PathBuf, name: String) -> Self {
        Self {
            path,
            name: Some(name),
            id: None,
        }
    }
}

/// Makes a folder on this host a project, so agents can be started in it from
/// any device, with or without the app.
pub async fn add_project(core: &Arc<Core>, params: AddProjectParams) -> CoreResult<ProjectInfo> {
    if !params.path.is_absolute() {
        return Err("a project's folder must be an absolute path".into());
    }
    let path = params.path;
    let path = blocking(move || {
        let path = sikemux_process::files::real_path(&path)
            .map_err(|error| CoreError::from(format!("{}: {error}", path.display())))?;
        if path.is_dir() {
            Ok(path)
        } else {
            Err(format!("{} is not a folder", path.display()).into())
        }
    })
    .await?;
    let project = core
        .workspaces()
        .add_project(path, params.name, params.id)?;
    (core.workspace_above().projects_changed)(core);
    Ok(project)
}

/// Stops offering a project, and closes its agents.
pub fn remove_project(core: &Core, params: RemoveProjectParams) -> CoreResult<()> {
    let before = core.workspaces().agent_ids();
    let id = core.workspaces().remove_project(&params.path)?;
    crate::server::new_project::forget(core, &id, &params.path);
    agents_closed(core, before);
    (core.workspace_above().project_removed)(core, &params.path.to_string_lossy());
    (core.workspace_above().projects_changed)(core);
    Ok(())
}

/// Renames, pins or moves a project to a space.
pub fn update_project(core: &Core, params: UpdateProjectParams) -> CoreResult<ProjectInfo> {
    let picks_changed = params.patch.contains_key("codeHosts");
    let project = core.workspaces().update_project(&params.id, params.patch)?;
    if let (true, Some(core)) = (picks_changed, core.arc()) {
        (core.workspace_above().code_hosts_picked)(&core, &project.path.to_string_lossy());
    }
    Ok(project)
}

/// Moves a project to `index` of the list as clients show it. A list sorted by recency or by
/// when projects were added becomes the order the person drags from then on.
pub fn move_project(core: &Core, params: MoveProjectParams) -> CoreResult<()> {
    if core.preferences().get().project_sort != ProjectSort::Manual {
        let shown: Vec<String> = ((core.workspace_above().workspace_view)(core)
            .projects
            .into_iter())
        .map(|project| project.id)
        .collect();
        core.workspaces().order_projects(&shown)?;
        crate::server::preferences::set(core, PreferenceChange::ProjectSort(ProjectSort::Manual))?;
    }
    core.workspaces().move_project(&params.id, params.index)
}

/// Puts `projects` in the order the person chose: the one worked in last first, the first
/// added first, or as they dragged them, the order they are kept in.
pub fn sort_projects(
    projects: &mut [ProjectInfo],
    sort: ProjectSort,
    last_active: &HashMap<String, u64>,
) {
    match sort {
        ProjectSort::Manual => {}
        ProjectSort::Created => projects.sort_by_key(|project| project.added_at),
        ProjectSort::Recent => projects.sort_by_key(|project| {
            std::cmp::Reverse(last_active.get(&project.id).copied().unwrap_or(0))
        }),
    }
}

pub fn set_spaces(core: &Core, params: SpacesParams) -> CoreResult<()> {
    core.workspaces().set_spaces(params.spaces)
}

/// Keeps the mode agents start in, and offers the launchers in it.
pub async fn set_defaults(core: &Arc<Core>, params: DefaultsParams) -> CoreResult<()> {
    if core
        .workspaces()
        .set_permission_mode(params.permission_mode)?
    {
        refresh_launchers(core.clone()).await;
    }
    core.changed(What::View);
    Ok(())
}
