//! Where a chat's turn stands, kept in one place under one lock, so what every
//! app is told of the chat and what its lead hears both follow from it.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Turn {
    #[default]
    Idle,
    /// A prompt this side sent is being answered.
    Prompted,
    /// The agent works on a turn nobody here prompted.
    Unprompted,
    /// The turn ended with subagents still at work, so it is not finished yet.
    Held,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(from = "Flags", into = "Flags")]
pub(super) struct TurnState {
    pub turn: Turn,
    /// A turn has started in this session, so its provider keeps it.
    pub turned: bool,
    /// A turn of this chat was answered, so its provider has it saved.
    pub answered: bool,
    /// A turn has started that the agent's lead has not yet heard the end of.
    pub unreported: bool,
    /// Why the turn in progress failed, said before the turn or the chat ends.
    pub failure: Option<String>,
    /// How many times a turn was held, so a wait on one hold leaves the next alone.
    pub holds: u64,
}

impl TurnState {
    /// A chat taken up again on a session its provider has saved, or not.
    pub fn saved(saved: bool) -> Self {
        Self {
            turned: saved,
            answered: saved,
            ..Self::default()
        }
    }

    pub fn running(&self) -> bool {
        matches!(self.turn, Turn::Prompted | Turn::Unprompted)
    }
}

/// How the turn stands, as a core that takes the chat over reads it.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Flags {
    running: bool,
    unprompted: bool,
    turned: bool,
    held: bool,
    answered: bool,
}

impl From<Flags> for TurnState {
    fn from(flags: Flags) -> Self {
        let turn = if flags.running {
            Turn::Prompted
        } else if flags.unprompted {
            Turn::Unprompted
        } else if flags.held {
            Turn::Held
        } else {
            Turn::Idle
        };
        Self {
            turn,
            turned: flags.turned,
            answered: flags.answered,
            unreported: turn != Turn::Idle,
            failure: None,
            holds: 0,
        }
    }
}

impl From<TurnState> for Flags {
    fn from(state: TurnState) -> Self {
        Self {
            running: state.turn == Turn::Prompted,
            unprompted: state.turn == Turn::Unprompted,
            turned: state.turned,
            held: state.turn == Turn::Held,
            answered: state.answered,
        }
    }
}
