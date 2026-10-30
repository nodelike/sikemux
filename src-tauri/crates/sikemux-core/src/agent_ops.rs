//! What the core does differently for each agent it knows. Each agent implements
//! `AgentOps` once, in `sessions/<agent>.rs`, and `AgentKind::ops` is the one place
//! that picks an implementation; what is only yes or no sits on its catalog entry.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use notify::RecursiveMode;
use serde_json::Value;

use crate::agent_catalog::AgentKind;
use crate::sessions::recent::{Found, Hit, PageScan};
use crate::sessions::search::Prose;
use crate::sessions::{self, AgentSession, FileStamp};

pub(crate) type Pending<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Asks the agent's CLI at `executable`, on the account `config_path` names, to rename a session.
pub(crate) type RenameCommand = for<'a> fn(
    executable: &'a Path,
    config_path: Option<&'a str>,
    session_id: &'a str,
    name: &'a str,
) -> Pending<'a, Result<(), String>>;

/// Asks the agent's CLI at `executable`, on the account `config_path` names, to delete a session.
pub(crate) type DeleteCommand = for<'a> fn(
    executable: &'a Path,
    config_path: Option<&'a str>,
    session_id: &'a str,
) -> Pending<'a, Result<(), String>>;

/// Where an agent writes the sessions of one folder and account.
#[derive(Default)]
pub(crate) struct SessionPlaces {
    /// Folders whose files each hold one session. Without its subfolders, only
    /// the files right in it count: Claude keeps a session's subagents beside it.
    pub(crate) sessions: Vec<(PathBuf, RecursiveMode)>,
    /// Files, or folders of files, that list the sessions together, so a change
    /// in one means listing them all again.
    pub(crate) listings: Vec<PathBuf>,
}

pub(crate) trait AgentOps: Sync {
    /// Its saved sessions that ran in `cwd`, newest first.
    fn sessions(&self, cwd: &str, config_path: Option<&str>) -> Vec<AgentSession>;

    /// Its sessions on one page of the Recent list.
    fn recent(&self, scan: &PageScan<'_>, config_path: Option<&str>) -> Vec<Hit>;

    /// What holds the session a file written under its session folders belongs to.
    /// None for agents that keep their sessions in a database.
    fn session_place(&self, _path: &Path) -> Option<PathBuf> {
        None
    }

    /// The session `place` holds, when it ran in `cwd`.
    fn session_at(
        &self,
        _place: &Path,
        _cwd: &str,
        _config_path: Option<&str>,
    ) -> Option<AgentSession> {
        None
    }

    /// Where it writes the sessions of `cwd` on the account `config_path` names.
    fn session_places(&self, cwd: &str, config_path: Option<&str>) -> SessionPlaces;

    /// A saved session as the updates its agent sends when it loads it, for agents
    /// whose chats show it before the agent starts.
    fn history(&self, _session_id: &str, _cwd: &Path) -> Result<Vec<Value>, String> {
        Ok(Vec::new())
    }

    /// The provider and model a saved session last ran on, as the agent names them,
    /// for agents that keep a provider beside each reply.
    fn last_model(&self, _session_id: &str) -> Option<(String, String)> {
        None
    }

    /// When what was said in `found` last changed.
    fn prose_stamp(&self, found: &Found) -> Option<FileStamp> {
        Some(sessions::file_stamp(found.place.as_deref()?))
    }

    /// What was said in `found`.
    fn prose(&self, found: &Found, prose: &mut Prose) -> Option<()>;

    /// Renames a session kept in files or a database Sikemux can write. None when
    /// only its CLI can.
    fn rename_saved(
        &self,
        _cwd: &str,
        _session_id: &str,
        _name: &str,
        _config_path: Option<&str>,
    ) -> Option<Result<(), String>> {
        None
    }

    /// How its CLI renames a session, when the name lives where only the CLI writes.
    fn rename_command(&self) -> Option<RenameCommand> {
        None
    }

    /// How its CLI deletes a session, when it can.
    fn delete_command(&self) -> Option<DeleteCommand> {
        None
    }

    /// Removes what it keeps of a deleted session beside the transcript, under `root`.
    fn delete_leftovers(&self, _root: &Path, _transcript: &Path, _session_id: &str) {}
}

impl AgentKind {
    pub(crate) fn ops(self) -> &'static dyn AgentOps {
        match self {
            AgentKind::Claude => &sessions::claude::Claude,
            AgentKind::Codex => &sessions::codex::Codex,
            AgentKind::Hermes => &sessions::hermes::Hermes,
            AgentKind::Pi => &sessions::pi::Pi,
            AgentKind::Opencode => &sessions::opencode::Opencode,
            AgentKind::Omp => &sessions::omp::Omp,
            AgentKind::Grok => &sessions::grok::Grok,
            AgentKind::Muse => &sessions::muse::Muse,
        }
    }
}
