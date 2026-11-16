//! What the board asks of the features built over it: messages between agents, questions,
//! memory, triggers, the chief and linked hosts. It cannot name those
//! features, so a core is built with these, one function for each.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::Value;
use sikemux_view::chat::{ChatTrigger, TriggerWhen};

use super::board::goal::Overrule;
use super::Core;

type Doing<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
/// A note on its way to an agent: what the agent's side answered, or why it could not be given.
type Told<'a> = Doing<'a, Result<Value, String>>;
/// A decision by its number and its text.
type Decision<'a> = (u64, &'a str);
type Undone =
    for<'a> fn(&'a Arc<Core>, &'a str, Option<u64>, Decision<'a>, Overrule) -> Doing<'a, ()>;

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
    /// What Sikemux keeps for the agent, for after its task in the same letter.
    /// Knowledge is picked by the words of `task`, a lead's own, when given, and
    /// of the agent's card.
    pub memory_brief: fn(&Core, &str, Option<&str>) -> Option<String>,
    /// Hands an agent a note from the board about its cards, with a line for the app.
    pub messages_from_board: for<'a> fn(&'a Arc<Core>, String, String, String) -> Told<'a>,
    /// Hands an agent the person's overruling of a decision as an answer.
    pub messages_overruled: for<'a> fn(&'a Arc<Core>, Overruled) -> Told<'a>,
    /// Whether a question the agent asked the person blocks it.
    pub questions_blocks_on_person: fn(&Core, &str) -> bool,
    /// The question of the agent's that blocks it on the person.
    pub questions_blocking: fn(&Core, &str) -> Option<String>,
    /// The boards changed, so open questions are looked at again.
    pub questions_boards_changed: fn(&Core),
    /// The short name a goal was given, once it has one.
    pub goal_title: fn(&Core, &str, u64) -> Option<String>,
    /// Asks for a short name for a new goal.
    pub goal_name_new: fn(&Arc<Core>, &str, u64, &str),
    /// When a repeating trigger is next due after `now`.
    pub triggers_next_after: fn(&TriggerWhen, u64, u64) -> Option<u64>,
    /// The triggers that add cards to the project's board.
    pub triggers_on_board: fn(&Core, &str) -> Vec<ChatTrigger>,
    /// An agent recorded a decision itself: its chat shows it, with Overrule.
    pub chief_decided: fn(&Core, &str, Option<u64>, &sikemux_view::slice::Decision),
    /// The person overruled a decision: the chief keeps what that undid.
    pub chief_overruled: fn(&Core, &str, Option<u64>, Decision<'_>, Vec<String>),
    /// What overruling the decision kept, when it can be undone.
    pub chief_undoable: fn(&Core, &str, Option<u64>, u64) -> Result<Overrule, String>,
    /// The overruling was undone: what it kept goes back.
    pub chief_overrule_undone: Undone,
    /// The board closed a card by itself once the project's check passed on `commit`: the chief's
    /// chat says so, with Undo.
    pub chief_card_closed: fn(&Core, &str, u64, &str, &str),
    /// The person reopened a card the board closed by itself.
    pub chief_card_reopened: fn(&Core, &str, u64),
    /// The agent that started the agent, when it runs on a linked host.
    pub hosts_started_by: fn(&Core, &str) -> Option<String>,
    /// The rows of linked hosts' agents changed.
    pub hosts_rows_moved: fn(&Core),
    /// The boards kept under these keys changed, for the project's other hosts to hear.
    pub records_changed: fn(&Arc<Core>, Vec<String>),
    /// The folders of this host's projects whose record another host keeps.
    pub records_elsewhere: fn(&Core) -> Vec<String>,
}
