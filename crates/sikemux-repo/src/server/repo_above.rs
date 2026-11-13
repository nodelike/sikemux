//! What the repository's modules ask of the features built over them: the board, messages
//! between agents, the roster, tasks and questions. They cannot name those features, so a core
//! is built with these, one function for each.

use std::collections::HashSet;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::Value;
use sikemux_calls::board::{BoardSettingsParams, Landing, ProjectBoard};
use sikemux_calls::questions::{AnswerParams, AnswersParams};
use sikemux_plugin_api::vault::Caller;
use sikemux_view::host_stats::HostEventKind;
use sikemux_view::turn::Landed;
use sikemux_wire::protocol::AgentState;

use super::chat::Saved;
use super::checks::{CheckRan, CiCard};
use super::codehost::ci::Shown;
use super::codehost::repo::FailedJob;
use super::listed::Listed;
use super::places::Place;
use super::ship::Deploy;
use super::trust_ask::TrustAsk;
use super::{Core, CoreResult};
use crate::cli::protocol::HarnessRequest;
use crate::tasks::config::ConfigLoad;

type Doing<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
/// A note on its way to an agent: what the agent's side answered, or why it could not be given.
type Told<'a> = Doing<'a, Result<Value, String>>;
type Note = for<'a> fn(&'a Arc<Core>, String, String) -> Told<'a>;
type Trusted = for<'a> fn(&'a Arc<Core>, &'a Path, Option<&'a str>) -> Doing<'a, bool>;
type Done<'a> = Doing<'a, Result<(), String>>;
type Settings =
    for<'a> fn(&'a Arc<Core>, BoardSettingsParams) -> Doing<'a, Result<ProjectBoard, String>>;
type AgentTool = for<'a> fn(&'a Arc<Core>, &'a str, Value, Option<Deploy>) -> Told<'a>;

pub struct RepoAbove {
    /// Whether the check stands for what `agent` deploys from `head`, the commit
    /// its checkout is at: on the card it works now, else on that commit as the
    /// board heard. Why not, when not.
    pub board_check_stands: fn(&Core, &str, &str, Option<&str>) -> Result<(), String>,
    /// The address a deploy to the place called `name` in `project` is read healthy at.
    pub board_health_of: fn(&Core, &str, &str) -> Option<String>,
    /// The project that holds the absolute path, or why the path will not do.
    pub board_project_at: fn(&Core, &str) -> Result<String, String>,
    /// `agent` made a commit: the run it works for, if any, has something new to keep.
    pub board_committed: fn(&Core, &str),
    /// A commit landed on `project`'s branch, so a role may have reached its threshold.
    pub board_landed: fn(&Core, &str),
    /// Every project's chief, by agent id.
    pub chief_all: fn(&Core) -> HashSet<String>,
    /// `agent` landed work in the project at `path` with a commit whose message is `message`.
    pub chief_landed: fn(&Core, &str, &str, &str),
    /// Whether question `id` asks for a change to production, which only its call answers.
    pub questions_asks_production: fn(&Core, &str) -> bool,
    /// The CLI the agent runs, or "agent" when it is not known.
    pub questions_cli_of: fn(&Core, &str) -> String,
    /// Every agent the core runs or the app holds, in the order the device view puts them.
    pub roster_listed: fn(&Core) -> Vec<Listed>,
    /// Hands an agent a note from Sikemux itself. Fails when the agent has stopped or is gone.
    pub messages_from_sikemux: Note,
    /// Hands an agent what the person said. Fails when the agent has stopped or is gone.
    pub messages_for_person: Note,
    /// Hands an agent what the person said, waiting for it when it has stopped.
    pub messages_for_person_waiting: Note,
    /// Hands an agent work from Sikemux, for the reason `why`.
    pub messages_work_from_sikemux: for<'a> fn(&'a Arc<Core>, String, String, &'a str) -> Told<'a>,
    /// Whether a note can reach the agent now.
    pub messages_reachable: for<'a> fn(&'a Arc<Core>, &'a str) -> Doing<'a, bool>,
    /// The state of the agent's chat, when it has one.
    pub agent_chat_state: fn(&Core, &str) -> Option<AgentState>,
    /// The result written on the card the agent works or was reviewed for.
    pub board_agent_result: fn(&Core, &str) -> Option<String>,
    /// The test results of the project's modules, as its tasks left them.
    pub tasks_module_tests: for<'a> fn(&'a Arc<Core>, String) -> Doing<'a, CoreResult<Value>>,
    /// Whether the project's file is trusted to run its commands for the agent.
    pub tasks_config_trusted: Trusted,
    /// Starts the agent's chat again in the checkout it was just given.
    pub launching_start_agent: for<'a> fn(&'a Arc<Core>, &'a str) -> Doing<'a, CoreResult<()>>,
    /// How many terminals run now, the spare ones aside.
    pub running_sessions: fn(&Core) -> usize,
    /// Notes an event in the host's history.
    pub host_note: fn(&Core, HostEventKind),
    /// Whether the agent's process runs now.
    pub agent_running: fn(&Core, &str) -> bool,
    /// What the person must trust before the project's file runs its commands, if anything.
    pub tasks_config_needs: fn(&Core, &ConfigLoad) -> Option<TrustAsk>,
    /// Asks the person to trust the commands, and says whether they did.
    pub tasks_ask_person:
        for<'a> fn(&'a Arc<Core>, &'a str, TrustAsk, Option<&'a str>) -> Doing<'a, bool>,
    /// Finds a saved session, of the agent kind when given, among every project's.
    pub saved_locate: fn(&Core, &str, Option<&str>) -> Result<Saved, String>,
    /// The repositories whose CI the board's cards wait on.
    pub board_waiting_on_ci: fn(&Core) -> Vec<Shown>,
    /// Runs moved in the repository, so the cards waiting on it look again.
    pub board_ci_moved: fn(&Core, &Shown),
    /// The repositories whose CI a trigger waits on.
    pub triggers_waiting_on_ci: fn(&Core) -> Vec<Shown>,
    /// Runs moved in the repository, so the triggers waiting on it look again.
    pub triggers_ci_moved: fn(&Arc<Core>, &Shown),
    /// The repositories of the pull requests the git panel shows.
    pub git_pending_pulls: fn(&Core) -> Vec<Shown>,
    /// Runs moved in the repository, so the git panel reads its pull requests again.
    pub git_ci_moved: fn(&Arc<Core>, &Shown),
    /// What the person set to count as landed in the project, when they did.
    pub board_landed_set: fn(&Core, &str) -> Option<Landed>,
    /// Changes the project's board settings as the person asked.
    pub board_person_settings: Settings,
    /// The project's places changed: its board's ladders follow them.
    pub board_mirror_places: fn(&Core, &str, Vec<Place>, Option<u64>),
    /// Who lands the project's work.
    pub board_landing: fn(&Core, &str) -> Landing,
    /// Adds a run of the project's check to the run record of each agent it took in.
    pub board_record_check: fn(&Core, &str, &CheckRan<'_>),
    /// CI ruled on a commit of the project: the jobs that failed, none when it passed.
    pub board_ci_ruled: fn(&Arc<Core>, &str, &str, &[FailedJob]),
    /// The latest commits of the project's unfinished slices that still wait for the check.
    pub board_unbuilt_heads: fn(&Core, &str) -> Vec<String>,
    /// The open card that holds the project's failing CI.
    pub board_ci_card: fn(&Core, &str) -> Option<CiCard>,
    /// Notes in the project's journal that its repository moved, by the kind of move.
    pub harness_note_repository: fn(&Core, &str, &str),
    /// Whether the agent asked, or was sent, a question whose answer stays hidden.
    pub questions_hears_hidden: fn(&Core, &str) -> bool,
    /// The agent's open questions that writing to it answers.
    pub questions_answered_by_writing: fn(&Core, &str) -> Vec<String>,
    /// Whether the question still waits for its answer.
    pub questions_is_open: fn(&Core, &str) -> bool,
    /// Answers one question as the person did.
    pub questions_answer: for<'a> fn(&'a Arc<Core>, AnswerParams) -> Done<'a>,
    /// Answers questions asked together.
    pub questions_answer_all: for<'a> fn(&'a Arc<Core>, AnswersParams) -> Done<'a>,
    /// Hands a question the person would not answer back to the lead it came through.
    pub questions_hand_back: for<'a> fn(&'a Arc<Core>, &'a str) -> Done<'a>,
    /// Who a plugin's vault sees calling it for this request.
    pub plugins_caller_of: fn(&Core, &HarnessRequest) -> Caller,
    /// Runs the agent tool, for the deploy it is when it is one.
    pub plugins_run_agent_tool: AgentTool,
}
