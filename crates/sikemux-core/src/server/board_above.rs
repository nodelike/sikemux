//! What the board asks of the features built over it: messages between agents, waking and
//! starting agents, questions, the inbox, linked hosts and the rest. It cannot name those
//! features, so a core is built with these, one function for each.

use std::any::Any;
use std::collections::HashSet;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::Value;
use sikemux_calls::harness::RunSelector;
use sikemux_view::chat::{ChatTrigger, TriggerWhen};
use sikemux_wire::protocol::AgentStatus;

use super::board::goal::Overrule;
use super::board::proof::ProofAsk;
use super::chat::Fault;
use super::remote::StoredLink;
use super::{Core, CoreResult};
use crate::cli::protocol::HarnessRequest;

type Doing<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
/// A note on its way to an agent: what the agent's side answered, or why it could not be given.
type Told<'a> = Doing<'a, Result<Value, String>>;
type Note = for<'a> fn(&'a Arc<Core>, String, String) -> Told<'a>;
type Woken = for<'a> fn(&'a Arc<Core>, String) -> Doing<'a, CoreResult<()>>;
type Task = for<'a> fn(&'a Arc<Core>, &'a str, String) -> Doing<'a, Result<(), String>>;
/// A turn that shows a page: its agent, when it began and the commit its work was at.
type Shown = (String, u64, Option<String>);
/// A decision by its number and its text.
type Decision<'a> = (u64, &'a str);
type Undone =
    for<'a> fn(&'a Arc<Core>, &'a str, Option<u64>, Decision<'a>, Overrule) -> Doing<'a, ()>;
type Handed =
    for<'a> fn(&'a Arc<Core>, &'a StoredLink, HandOver) -> Doing<'a, Result<String, String>>;
/// A checkout packed to go to another host, as hosts pack it.
pub type Packed = Box<dyn Any + Send>;

/// An agent on a linked host, as far as the board looks at it.
pub struct ForeignNow {
    pub agent_id: String,
    /// The key of the host it runs on.
    pub host: String,
    pub status: AgentStatus,
    /// It is a worker, not a lead's stand-in.
    pub worker: bool,
    /// The agent that started it.
    pub parent: Option<String>,
}

/// A builder Auto hands to another host.
pub struct HandOver {
    pub agent_id: String,
    pub name: String,
    pub provider: String,
    pub project: String,
    /// The agent Auto goes by there, and the permission mode it runs in.
    pub lead: String,
    pub mode: String,
    pub model: Option<String>,
    pub prompt: String,
    pub branch: String,
    pub packed: Packed,
}

/// An answer the person gave, for the agent it goes to.
pub struct Overruled {
    pub question: String,
    pub asker: String,
    pub asker_id: String,
    pub cli: String,
    pub text: String,
    pub note: String,
}

pub struct BoardAbove {
    /// Stops a worker this host started on another, whenever that host next hears, and
    /// forgets it here. Whatever it sends after this is not taken.
    pub hosts_stop_worker: fn(&Core, &str),
    /// What Sikemux keeps for the agent, for after its task in the same letter.
    /// Knowledge is picked by the words of `task`, a lead's own, when given, and
    /// of the agent's card.
    pub memory_brief: fn(&Core, &str, Option<&str>) -> Option<String>,
    /// The task Auto gives an agent it started, as that agent reads it.
    pub messages_auto_task: fn(&Core, &str, String) -> String,
    /// Types what the person wrote to a terminal agent into its terminal once its CLI takes a
    /// prompt, waking it on its saved session first when its terminal has ended.
    pub messages_from_person: fn(&Arc<Core>, &str, String) -> CoreResult<()>,
    /// Takes the page the agent's checkout serves at each width, for its turn that began at `turn`
    /// with its work at `head`, once per turn, off the caller's path; nothing when nothing serves it.
    pub pictures_take: fn(&Core, &str, &str, u64, Option<String>),
    /// Reads again where the agent's work is in git, off the caller's thread, and tells clients
    /// when that moved.
    pub turn_state_read_work: fn(&Core, &str),
    /// Hands an agent a note from the board about its cards, with a line for the app.
    pub messages_from_board: for<'a> fn(&'a Arc<Core>, String, String, String) -> Told<'a>,
    /// Hands an agent the task it was started for.
    pub messages_hand_task: Task,
    /// Hands an agent a note from Auto.
    pub messages_from_auto: Note,
    /// Hands an agent the person's overruling of a decision as an answer.
    pub messages_overruled: for<'a> fn(&'a Arc<Core>, Overruled) -> Told<'a>,
    /// Wakes the agent, a chat or a terminal.
    pub launching_wake: Woken,
    /// Wakes the agent's chat and waits until it takes a prompt.
    pub launching_wake_chat: Woken,
    /// Opens a closed agent again on the session it ran.
    pub launching_reopen_agent: for<'a> fn(&'a Arc<Core>, String) -> Doing<'a, CoreResult<String>>,
    /// Whether a question the agent asked the person blocks it.
    pub questions_blocks_on_person: fn(&Core, &str) -> bool,
    /// Whether the agent has a question open.
    pub questions_has_open: fn(&Core, &str) -> bool,
    /// The question of the agent's that blocks it on the person.
    pub questions_blocking: fn(&Core, &str) -> Option<String>,
    /// The boards changed, so open questions are looked at again.
    pub questions_boards_changed: fn(&Core),
    /// Puts a slice's proof to the person as a question from the agent.
    pub questions_put_proof_to_person:
        fn(&Core, &str, ProofAsk, String, String) -> Result<(), String>,
    /// The agents that have a run of a task going.
    pub harness_running_agents: fn(&Core) -> HashSet<String>,
    /// Stops the runs the selector picks.
    pub harness_stop_runs: for<'a> fn(&'a Arc<Core>, RunSelector) -> Doing<'a, ()>,
    /// Passes a request to the window and gives its answer.
    pub harness_forward: for<'a> fn(&'a Core, HarnessRequest) -> Told<'a>,
    /// Whether the agent's turn ended on a fault.
    pub inbox_failing: fn(&Core, &str) -> bool,
    /// The fault the agent's turn ended on.
    pub inbox_fault: fn(&Core, &str) -> Option<Fault>,
    /// The agent committed, so its turn's handoff is settled.
    pub inbox_committed: fn(&Core, &str),
    /// The turns of the project's agents that show a page, but for the covered ones.
    pub inbox_showing: fn(&Core, &str, &[String]) -> Vec<Shown>,
    /// The agent's turn is shown at this commit.
    pub inbox_staged: fn(&Core, &str, String),
    /// The short name a goal was given, once it has one.
    pub goal_title: fn(&Core, &str, u64) -> Option<String>,
    /// Asks for a short name for a new goal.
    pub goal_name_new: fn(&Arc<Core>, &str, u64, &str),
    /// When a repeating trigger is next due after `now`.
    pub triggers_next_after: fn(&TriggerWhen, u64, u64) -> Option<u64>,
    /// The triggers that add cards to the project's board.
    pub triggers_on_board: fn(&Core, &str) -> Vec<ChatTrigger>,
    /// The person overruled a decision: the chief keeps what that undid.
    pub chief_overruled: fn(&Core, &str, Option<u64>, Decision<'_>, bool, Vec<String>),
    /// What overruling the decision kept, when it can be undone.
    pub chief_undoable: fn(&Core, &str, Option<u64>, u64) -> Result<Overrule, String>,
    /// The overruling was undone: what it kept goes back.
    pub chief_overrule_undone: Undone,
    /// Whether the linked host can be reached now.
    pub hosts_reachable: fn(&Core, &str) -> bool,
    /// The agent, when it runs on a linked host.
    pub foreign: fn(&Core, &str) -> Option<ForeignNow>,
    /// Every agent on a linked host.
    pub foreign_all: fn(&Core) -> Vec<ForeignNow>,
    /// The branch the agent's latest commits came back on, taken once.
    pub foreign_take_landed: fn(&Core, &str) -> Option<String>,
    /// The agent on a linked host is gone for good.
    pub foreign_gone: fn(&Core, &str),
    /// The link to the host of that name or key.
    pub hosts_find: fn(&Core, &str) -> Result<StoredLink, String>,
    /// The name the linked host goes by.
    pub hosts_name_of: fn(&Core, &str) -> String,
    /// Packs the checkout to go to another host, as the project `folder` when given.
    pub hosts_package: fn(&Path, Option<String>) -> Result<Packed, String>,
    /// Hands a builder to the linked host and gives the name it runs under there.
    pub hosts_hand_over: Handed,
    /// The rows of linked hosts' agents changed.
    pub hosts_rows_moved: fn(&Core),
}
