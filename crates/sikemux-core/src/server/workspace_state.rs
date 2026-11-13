//! What the core keeps of the workspace and the state beside it, and how the rest reads it.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::changed::{ChangedCore, What};
use super::serving::ServeCore;
use super::{
    activity_log, diagrams, drafts, faces, names, new_project, open_files, preferences, seen,
    workspace, Core,
};

pub(crate) struct WorkspaceFields {
    pub(crate) workspaces: workspace::Workspaces,
    pub(crate) names: names::Names,
    /// The repositories coming down into new projects.
    pub(crate) clones: new_project::Clones,
    pub(crate) preferences: preferences::Prefs,
    pub(crate) activity_log: Arc<activity_log::ActivityLog>,
    /// The files clients have open, watched wherever they are.
    pub(crate) open_files: open_files::OpenFiles,
    /// What the person has typed and not sent yet, by what it is for.
    pub(crate) drafts: drafts::Drafts,
    /// Where the person moved each diagram's tables, by what it is a diagram of.
    pub(crate) diagrams: diagrams::Diagrams,
    pub(crate) seen: seen::Seen,
    /// The faces agents wear, and the style the person picked for them.
    pub(crate) faces: faces::Faces,
    pub(crate) workspace_above: super::workspace_above::WorkspaceAbove,
    /// Where files pasted into a chat are kept.
    pub(crate) attachment_dir: Option<PathBuf>,
}

pub(crate) trait WorkspaceParts {
    fn workspaces(&self) -> &workspace::Workspaces;
    fn names(&self) -> &names::Names;
    fn clones(&self) -> &new_project::Clones;
    fn preferences(&self) -> &preferences::Prefs;
    fn activity_log(&self) -> &Arc<activity_log::ActivityLog>;
    fn open_files(&self) -> &open_files::OpenFiles;
    fn drafts(&self) -> &drafts::Drafts;
    fn diagrams(&self) -> &diagrams::Diagrams;
    fn seen(&self) -> &seen::Seen;
    fn faces(&self) -> &faces::Faces;
    fn workspace_above(&self) -> &super::workspace_above::WorkspaceAbove;
    fn attachment_dir(&self) -> &Option<PathBuf>;
}

impl WorkspaceParts for Core {
    fn workspaces(&self) -> &workspace::Workspaces {
        &self.kept::<WorkspaceFields>().workspaces
    }

    fn names(&self) -> &names::Names {
        &self.kept::<WorkspaceFields>().names
    }

    fn clones(&self) -> &new_project::Clones {
        &self.kept::<WorkspaceFields>().clones
    }

    fn preferences(&self) -> &preferences::Prefs {
        &self.kept::<WorkspaceFields>().preferences
    }

    fn activity_log(&self) -> &Arc<activity_log::ActivityLog> {
        &self.kept::<WorkspaceFields>().activity_log
    }

    fn open_files(&self) -> &open_files::OpenFiles {
        &self.kept::<WorkspaceFields>().open_files
    }

    fn drafts(&self) -> &drafts::Drafts {
        &self.kept::<WorkspaceFields>().drafts
    }

    fn diagrams(&self) -> &diagrams::Diagrams {
        &self.kept::<WorkspaceFields>().diagrams
    }

    fn seen(&self) -> &seen::Seen {
        &self.kept::<WorkspaceFields>().seen
    }

    fn faces(&self) -> &faces::Faces {
        &self.kept::<WorkspaceFields>().faces
    }

    fn workspace_above(&self) -> &super::workspace_above::WorkspaceAbove {
        &self.kept::<WorkspaceFields>().workspace_above
    }

    fn attachment_dir(&self) -> &Option<PathBuf> {
        &self.kept::<WorkspaceFields>().attachment_dir
    }
}

/// The agents the workspace knows of, and their names.
pub(crate) trait WorkspaceCore {
    fn running_agents(&self) -> HashSet<String>;

    /// The agents that keep their names: those running here and those the
    /// workspace holds, asleep or not.
    fn open_agents(&self) -> HashSet<String>;

    /// The agent's name, given now when it has none. A closed agent opening again under its
    /// id comes back as itself, with the buddy it wore, which it answers too. Reads and writes
    /// the disk, so it belongs on a blocking thread.
    fn name_agent(
        &self,
        agent_id: &str,
        cwd: &Path,
        provider: Option<&str>,
        resume_id: Option<&str>,
    ) -> (names::AgentName, Option<names::Returned>);
}

impl WorkspaceCore for Core {
    fn running_agents(&self) -> HashSet<String> {
        let mut running: HashSet<String> = (self.workspace_above().chat_agents)(self)
            .into_iter()
            .collect();
        running.extend(self.all_sessions().iter().filter_map(|session| {
            session
                .agent
                .as_ref()
                .map(|agent| agent.agent_id().to_owned())
        }));
        running
    }

    fn open_agents(&self) -> HashSet<String> {
        let mut open = self.running_agents();
        open.extend(self.workspaces().agent_ids());
        open
    }

    fn name_agent(
        &self,
        agent_id: &str,
        cwd: &Path,
        provider: Option<&str>,
        resume_id: Option<&str>,
    ) -> (names::AgentName, Option<names::Returned>) {
        let starting = names::Starting {
            agent_id,
            project: self.workspaces().project_of(cwd),
            provider,
            resume_id,
        };
        let reopened = (self.names()).come_back(agent_id, &starting.project, self.open_agents());
        if let Some(buddy) = reopened.as_ref().and_then(|back| back.buddy.as_ref()) {
            let _ = self
                .workspaces()
                .edit_agent(agent_id, |agent| agent.buddy = Some(buddy.clone()));
        }
        let (named, changed) = self.names().give(starting, self.open_agents());
        if let Some(agent) = self.workspaces().agent(agent_id) {
            (self.names()).note_runs(vec![(agent.id, agent.profile_id, agent.terminal)]);
        }
        if changed || reopened.is_some() {
            self.changed(What::View);
        }
        (named, reopened)
    }
}
