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
use sikemux_wire::protocol::AgentStatus;

use super::chat::Saved;
use super::checks::CiCard;
use super::codehost::ci::Shown;
use super::codehost::repo::FailedJob;
use super::listed::Listed;
use super::places::Place;
use super::ship::Deploy;
use super::trust_ask::TrustAsk;
use super::{Core, CoreResult};
use crate::cli::protocol::HarnessRequest;
use crate::tasks::config::ConfigLoad;
use sikemux_view::calls::PersonCall;

/// A webhook of this host just made, and where senders post to it.
pub struct NewHook {
    pub id: String,
    pub url: String,
}

type Doing<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
/// A note on its way to an agent: what the agent's side answered, or why it could not be given.
type Told<'a> = Doing<'a, Result<Value, String>>;
type Note = for<'a> fn(&'a Arc<Core>, String, String) -> Told<'a>;
type Trusted = for<'a> fn(&'a Arc<Core>, &'a Path, Option<&'a str>) -> Doing<'a, bool>;
type Done<'a> = Doing<'a, Result<(), String>>;
type Settings =
    for<'a> fn(&'a Arc<Core>, BoardSettingsParams) -> Doing<'a, Result<ProjectBoard, String>>;
type Committed = fn(&Core, &str, &str, &[(String, String)], bool);
type AgentTool = for<'a> fn(&'a Arc<Core>, &'a str, Value, Option<Deploy>) -> Told<'a>;

pub struct RepoAbove {
    /// The address a deploy to the place called `name` in `project` is read healthy at.
    pub board_health_of: fn(&Core, &str, &str) -> Option<String>,
    /// The project that holds the absolute path, or why the path will not do.
    pub board_project_at: fn(&Core, &str) -> Result<String, String>,
    /// Every project's chief, by agent id.
    pub chief_all: fn(&Core) -> HashSet<String>,
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
    /// Tells the agent's lead, when it has one, how its land went, calmly or at once.
    pub messages_tell_lead: fn(&Arc<Core>, &str, String, bool),
    /// Whether a note can reach the agent now.
    pub messages_reachable: for<'a> fn(&'a Arc<Core>, &'a str) -> Doing<'a, bool>,
    /// The id a note reaches the agent by now, reopening it as it was when it had closed here.
    pub messages_reopened: for<'a> fn(&'a Arc<Core>, &'a str) -> Doing<'a, Option<String>>,
    /// Sends a worker's commits back to its lead on another host to land there, saying so; none
    /// when the agent is not such a worker.
    pub hosts_land_at_lead: fn(&Core, &str, bool) -> Option<Result<String, String>>,
    /// The status of an agent that runs on a linked host, as that host last said it.
    pub foreign_status: fn(&Core, &str) -> Option<AgentStatus>,
    /// The watch credited the agent with files it wrote outside an edit tool, which can come to
    /// light after its turn ended.
    pub turn_state_wrote: fn(&Core, &str),
    /// The agent committed or its commits landed, so where its turn's work is is read again.
    /// Commits went to a remote, so what is on its branches moved.
    pub pushed: fn(&Core),
    /// Commits the agents made went to the remote from the checkout, the newest given, so
    /// its CI is watched for them.
    pub triggers_pushed: fn(&Core, &Path, &[String], &str),
    /// When the agent's last turn ended done, with nothing left to bring the agent back.
    pub turn_state_done_at: fn(&Core, &str) -> Option<u64>,
    pub turn_state_committed: fn(&Core, &str),
    /// The project's check passed on the `covered` commits, so the pages of the turns whose
    /// work is at one of them are taken.
    pub pictures_checked: fn(&Core, &str, &[String]),
    /// A deploy of the agent's work at the commit went to a place before production, and
    /// passed.
    pub inbox_staged: fn(&Core, &str, String),
    /// Whether the agent waits on the answer to a question it asked.
    pub questions_waits_on_answer: fn(&Core, &str) -> bool,
    /// Whether a message is on its way to the agent.
    pub mail_waiting: fn(&Core, &str) -> bool,
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
    /// What the person must trust before the project's file runs its commands, if anything.
    pub tasks_config_needs: fn(&Core, &ConfigLoad) -> Option<TrustAsk>,
    /// Asks the person to trust the commands, and says whether they did.
    pub tasks_ask_person:
        for<'a> fn(&'a Arc<Core>, &'a str, TrustAsk, Option<&'a str>) -> Doing<'a, bool>,
    /// Finds a saved session, of the agent kind when given, among every project's.
    pub saved_locate: fn(&Core, &str, Option<&str>) -> Result<Saved, String>,
    /// The repositories whose CI a trigger waits on.
    pub triggers_waiting_on_ci: fn(&Core) -> Vec<Shown>,
    /// Runs moved in the repository, so the triggers waiting on it look again.
    pub triggers_ci_moved: fn(&Arc<Core>, &Shown),
    /// A webhook of this host on the accounts server: its id, and the address senders post to.
    pub hooks_register: for<'a> fn(&'a Arc<Core>) -> Doing<'a, CoreResult<NewHook>>,
    /// Takes one of this host's webhooks away.
    pub hooks_remove: for<'a> fn(&'a Core, &'a str) -> Doing<'a, CoreResult<()>>,
    /// The repositories of the pull requests the git panel shows.
    pub git_pending_pulls: fn(&Core) -> Vec<Shown>,
    /// Runs moved in the repository, so the git panel reads its pull requests again.
    pub git_ci_moved: fn(&Arc<Core>, &Shown),
    /// What the person set to count as landed in the project, when they did.
    pub board_landed_set: fn(&Core, &str) -> Option<Landed>,
    /// Changes the project's board settings as the person asked.
    pub board_person_settings: Settings,
    /// The project's places changed: its board keeps them for clients.
    pub board_mirror_places: fn(&Core, &str, Vec<Place>, Option<u64>),
    /// Who lands the project's work.
    pub board_landing: fn(&Core, &str) -> Landing,
    /// CI ruled on a commit of the project: the jobs that failed, none when it passed.
    pub board_ci_ruled: fn(&Arc<Core>, &str, &str, &[FailedJob]),
    /// The open card that holds the project's failing CI.
    pub board_ci_card: fn(&Core, &str) -> Option<CiCard>,
    /// The project's check passed on `commit`, on its branch, which holds these agents' commits.
    pub board_check_passed: fn(&Arc<Core>, &str, &[String], &str),
    /// The agent made these commits, each a sha and subject, oldest first, and whether they are
    /// on the project's branch.
    pub board_committed: Committed,
    /// Notes in the project's journal that its repository moved, by the kind of move.
    pub harness_note_repository: fn(&Core, &str, &str),
    /// Whether the question still waits for its answer.
    pub questions_is_open: fn(&Core, &str) -> bool,
    /// Answers one question as the person did.
    pub questions_answer: for<'a> fn(&'a Arc<Core>, AnswerParams) -> Done<'a>,
    /// Answers questions asked together.
    pub questions_answer_all: for<'a> fn(&'a Arc<Core>, AnswersParams) -> Done<'a>,
    /// Who a plugin's vault sees calling it for this request.
    pub plugins_caller_of: fn(&Core, &HarnessRequest) -> Caller,
    /// Runs the agent tool, for the deploy it is when it is one.
    pub plugins_run_agent_tool: AgentTool,
    /// The person answered a call about chats an account's provider refused, keyed `key`.
    pub accounts_call_answered: for<'a> fn(&'a Arc<Core>, &'a PersonCall, &'a str) -> Doing<'a, ()>,
    /// The person kept or dropped an entry waiting on an org's list, keyed `key`.
    pub memory_call_answered: fn(&Arc<Core>, &PersonCall, &str),
}
