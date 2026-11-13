//! What chats ask of the features built over them. They cannot name those features, so a core
//! is built with these, one function for each.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use serde_json::Value;
use sikemux_view::questions::QuestionChoice;
use sikemux_view::slice::Decision;
use sikemux_view::turn::TurnState;
use sikemux_wire::protocol::{ChatContext, ChatLaunch};

use crate::agent_accounts::usage::UsageWindow;

use super::chat::feed::Feed;
use super::chat::rotation::KeptSession;
use super::chat::Chat;
use super::{Core, CoreResult};

/// An agent, the dialog's id, its header, its text and what the person may choose.
type AskForDialog = fn(&Core, &str, String, &str, &str, Vec<QuestionChoice>) -> Result<(), String>;

pub(crate) struct ChatAbove {
    /// A chat on `account`, one of the person's profiles or the default one when absent, was
    /// refused for sign-in: the account is signed out now, whatever its CLI last said.
    pub agent_accounts_chat_refused: fn(&Core, &str, Option<&str>),
    /// `board::chat_goal::subagent_started`.
    pub board_subagent_started: fn(&Core, &str),
    /// What the agent is told with the person's next message, once. Being told its work looks like a
    /// goal no longer applies once the chat has one.
    pub board_take_note: fn(&Core, &str) -> Option<ChatContext>,
    /// A chat taken up again on the provider session of an owner that lost its slices holds them
    /// again, unless someone took them meanwhile, and answers with where its goal and card stand,
    /// for it to be handed with its next message.
    pub board_resumed_session: fn(&Core, &str, &str, &str) -> Option<String>,
    /// A usage limit's wait gave up, so a run it held waits on the person after all.
    pub board_stopped_waiting: fn(&Core, &str),
    /// What the chief is handed as it starts: how it works, then the project's rules and knowledge.
    pub chief_brief: fn(&Core, &Path) -> String,
    /// The chief's chat opened on `feed`: what happened while the person was away is shown on it.
    pub chief_chat_opened: fn(&Core, &str, &Arc<Feed>),
    /// The chief's chat is ready and its saved history shown: the lines it keeps that this feed has
    /// not had go at its end, each under the time it was first made.
    pub chief_chat_ready: fn(&Core, &Chat),
    /// Keeps where a chief's session stands when it changed, so a sleep or a new core goes on from it.
    pub chief_keep_session: fn(&Core, &Chat),
    /// `chief::kept_session`.
    pub chief_kept_session: fn(&Core, &str) -> Option<KeptSession>,
    /// The person wrote to `agent_id` themselves. Written to a chief, they are with it, and an away
    /// block with no call in it is done with; written to another agent, its chief's next digest
    /// reads that chat.
    pub chief_person_wrote: fn(&Core, &str),
    /// `hosts::name_of`.
    pub hosts_name_of: fn(&Core, &str) -> String,
    /// Something the agents here show changed: the hosts their teammates run on hear each row
    /// that differs from what they were last told.
    pub hosts_rows_moved: fn(&Core),
    /// Gives a chat about to start the tools that are its own: those the launch
    /// offers in its environment, and only for its agent.
    pub launching_own_tools: fn(&Core, &mut ChatLaunch),
    /// Types what the person wrote to a terminal agent into its terminal once its CLI takes a
    /// prompt, waking it on its saved session first when its terminal has ended.
    pub messages_from_person: fn(&Arc<Core>, &str, String) -> CoreResult<()>,
    /// The person wrote to the agent, so it may take turns of its own again, and what waited for
    /// them goes on.
    pub messages_person_wrote: fn(&Core, &str),
    /// The agent called a tool, by the name its CLI gives it.
    pub messages_used_tool: fn(&Core, &str, &str),
    /// The agent did something besides messaging other agents, which ends any
    /// loop it was in: the messages held between it and another agent go on.
    pub messages_worked: fn(&Core, &str),
    /// Whether what the person types to `agent_id` may answer a call whose words are never kept: one
    /// it waits on, or a question it asked or a worker of its asked through it, which the person may
    /// as well answer in the lead's chat. Such a question counts with no call open here, as when a
    /// linked host took it to put to the person there.
    pub person_calls_hides_typed: fn(&Core, &str) -> bool,
    /// What the plugins set for an agent, terminal or task started in `cwd`,
    /// asked of the project that holds it.
    pub plugins_agent_environment: fn(&Core, &Path) -> BTreeMap<String, String>,
    /// Takes a question asked with a CLI's own tool, such as Claude's
    /// AskUserQuestion or Codex's request_user_input, into the same questions,
    /// and says what the agent is told in place of an answer.
    pub questions_ask_from_cli: fn(&Arc<Core>, &str, &Value) -> String,
    /// The chat's turn ended by itself: an offer of more work in its reply, which a yes or a no
    /// answers, sends the agent back when a project rule covers it, and otherwise only shows as how
    /// the turn left things. An agent with a lead, here or on a linked host, or that Auto runs,
    /// answers to them instead, and one with a message still on its way is not done.
    pub questions_offers_in_reply: fn(&Arc<Core>, &str),
    /// Closes the question on `agent_id`'s dialog `only`, or on all its dialogs,
    /// when they no longer wait.
    pub questions_withdraw_dialogs: fn(&Core, &str, Option<&str>),
    /// Puts the chat's triggers in its feed, as it starts or wakes.
    pub triggers_seed: fn(&Core, &Feed, &str),
    /// How old a reading may grow before it says so: three of the app's reads missed.
    /// Keeps the plan limits a chat's agent reported on the account `account_id` as that account's
    /// reading, telling clients when it changed.
    pub agent_accounts_note_live: fn(&Core, &str, Option<&str>, Vec<UsageWindow>),
    /// The project's decisions still standing that were made since `since`, newest first, at
    /// most `most`: every goal's, and those made on no goal.
    pub board_standing_since: fn(&Core, &str, u64, usize) -> Vec<Decision>,
    /// What Sikemux keeps for the agent, for after its task in the same letter.
    /// Knowledge is picked by the words of `task`, a lead's own, when given, and
    /// of the agent's card.
    pub memory_brief: fn(&Core, &str, Option<&str>) -> Option<String>,
    /// What a chat the person starts in `cwd` is handed as it begins: the
    /// project's rules and the knowledge that names nothing in particular. A
    /// goal's rules and decisions, and knowledge about what its work names, come
    /// later through `memory_list`.
    pub memory_for_chat: fn(&Core, &Path) -> Option<String>,
    /// Asks the person something a dialog in `agent_id`'s process is blocked on.
    pub questions_ask_for_dialog: AskForDialog,
    /// How `agent_id`'s turn stands, worked out now from its own facts alone.
    pub turn_state_now: fn(&Core, &str) -> Option<TurnState>,
}
