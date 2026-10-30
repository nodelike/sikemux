//! What the core does differently for each agent it knows. Each agent implements
//! `AgentOps` once, in `sessions/<agent>.rs`, and `AgentKind::ops` is the one place
//! that picks an implementation; what is only yes or no sits on its catalog entry.

use std::collections::BTreeMap;
use std::future::{ready, Future};
use std::path::{Path, PathBuf};
use std::pin::Pin;

use notify::RecursiveMode;
use serde_json::Value;

use crate::agent_accounts::sign_in::SignIn;
use crate::agent_accounts::status::{AccountStatus, AgentStatus};
use crate::agent_accounts::usage::Usage;
use crate::agent_catalog::AgentKind;
use crate::git_guard::homes::GitHook;
use crate::launch::providers::ChatRoute;
use crate::launch::tools::{TerminalSetup, Tools};
use crate::server::activity_log::{ModelTally, ReadState};
use crate::server::chat::feed::Said;
use crate::sessions::context::SessionContext;
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

/// Counts the tokens in transcript text its CLI wrote, read on from `state`.
pub(crate) type TokenCounter = fn(text: &str, state: ReadState) -> (ModelTally, ReadState);

/// What one record of the transcript its CLI writes says, each with whether a turn starts there.
pub(crate) type SaidReader = fn(record: &Value, items: &mut Vec<(bool, Said)>);

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
    /// The folder it keeps its settings and sessions in, for an agent whose catalog
    /// entry names no home.
    fn home(&self) -> Option<PathBuf> {
        None
    }

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

    /// The transcript of session `id` under the account folder `root`, looked for in
    /// `cwd`'s project first, for agents whose transcripts Sikemux reads line by line.
    fn transcript(&self, _root: &Path, _cwd: Option<&str>, _id: &str) -> Option<PathBuf> {
        None
    }

    /// How full the context window of the session in `transcript` was when it was last written.
    fn context(&self, _transcript: &Path) -> Option<SessionContext> {
        None
    }

    /// The folder under `root` it files the sessions of `cwd` in, for an agent that files
    /// each project's sessions apart and keeps a folder named by the session, for its
    /// subagents, beside the transcript.
    fn project_folder(&self, _root: &Path, _cwd: &str) -> Option<PathBuf> {
        None
    }

    /// The arguments it starts with in a terminal to reach Sikemux's tools, put in front
    /// of its own, adding what it reads from its environment to `setup`.
    fn terminal_tools<'a>(
        &'a self,
        setup: &'a mut TerminalSetup<'_>,
    ) -> Pending<'a, Result<Vec<String>, String>>;

    /// A private home holding the skills `library` for a chat, as the variable that
    /// names it and the folder, for agents that read skills only from their home.
    fn chat_home<'a>(
        &self,
        _tools: &'a Tools,
        _agent_id: &'a str,
        _program: &'a str,
        _library: &'a Path,
    ) -> Option<Pending<'a, Result<(String, String), String>>> {
        None
    }

    /// Gives a chat the skills library in `data_dir`, at `library`, through its
    /// arguments or environment.
    fn chat_skills(
        &self,
        _data_dir: &Path,
        _library: &Path,
        _args: &mut Vec<String>,
        _env: &mut BTreeMap<String, String>,
    ) {
    }

    /// The model it starts with when Sikemux names none, read from its own configuration.
    fn default_model(&self, _config_path: Option<&str>) -> Option<String> {
        None
    }

    /// The reasoning effort it starts with when Sikemux names none, read from its own
    /// configuration.
    fn default_effort(&self, _config_path: Option<&str>) -> Option<String> {
        None
    }

    /// Whether an account folder Sikemux makes links `name` to the default account's.
    fn shared_by_accounts(&self, _name: &str) -> bool {
        false
    }

    /// Writes what a new account folder at `directory` starts with, taken from the default
    /// account's under `home`.
    fn seed_account(&self, _home: &Path, _directory: &Path) -> Result<(), String> {
        Ok(())
    }

    /// Who an account is signed in as, asked of its CLI at `program`, for agents that
    /// keep separate accounts.
    fn account<'a>(
        &self,
        _program: &'a Path,
        _config_path: Option<&'a str>,
    ) -> Option<Pending<'a, Result<AccountStatus, String>>> {
        None
    }

    /// Whether it is signed in, asked of its CLI at `program` with the connected model
    /// `providers`' keys. Unknown for agents that cannot say: Hermes answers in free text
    /// that changes with the provider, and others have no check, so a chat refused for
    /// sign-in marks them signed out instead.
    fn sign_in_status<'a>(
        &self,
        _program: &'a Path,
        _config_path: Option<&'a str>,
        _providers: BTreeMap<String, String>,
    ) -> Pending<'a, AgentStatus> {
        Box::pin(ready(AgentStatus::Unknown))
    }

    /// The plan usage of an account, as its CLI at `program` reports it.
    fn usage<'a>(
        &self,
        _program: &'a Path,
        _config_path: Option<&'a str>,
    ) -> Option<Pending<'a, Result<Usage, String>>> {
        None
    }

    /// Who it is signed in as, read from the files its CLI writes, with `env` naming the
    /// account's folder.
    fn signed_in(&self, _env: &BTreeMap<String, String>) -> Option<SignIn> {
        None
    }

    /// Points it, an agent that speaks chat completions, at `route`'s provider, through
    /// its environment and arguments.
    fn route_chat(
        &self,
        _route: &ChatRoute<'_>,
        _env: &mut BTreeMap<String, String>,
        _args: &mut Vec<String>,
    ) {
    }

    /// Writes what it needs before it is pointed at a provider: for OMP, the extension
    /// at `omp_extension` that tells it of one.
    fn prepare_route(&self, _omp_extension: &Path) -> std::io::Result<()> {
        Ok(())
    }

    /// The plugin its ACP sessions load, beside their folder and tools.
    fn acp_plugin(&self) -> Option<&'static Path> {
        None
    }

    /// How it counts a turn's tokens from its transcript, for agents that report them
    /// only there.
    fn transcript_tokens(&self) -> Option<TokenCounter> {
        None
    }

    /// How what an agent in a terminal said is read from its transcript.
    fn transcript_said(&self) -> Option<SaidReader> {
        None
    }

    /// Whether its transcript marks where a turn ends. When it does not, its next
    /// prompt ends the turn before.
    fn transcript_ends_turns(&self) -> bool {
        true
    }

    /// The arguments its CLI signs an account in with, for agents that keep separate accounts.
    fn account_sign_in(&self) -> Option<&'static [&'static str]> {
        None
    }

    /// The arguments its CLI signs an account out with.
    fn account_sign_out(&self) -> Option<&'static [&'static str]> {
        None
    }

    /// How it is given the git hook, for an agent that takes hooks only from its
    /// home's config; `terminal` when it runs in a terminal rather than a chat.
    fn git_hook(&self, _terminal: bool) -> Option<GitHook> {
        None
    }

    /// Its own global skills folder, whether or not it exists yet.
    fn skills_folder(&self) -> Option<PathBuf> {
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
