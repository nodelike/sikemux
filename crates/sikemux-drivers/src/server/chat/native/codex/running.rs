//! What a Codex thread has going besides its own turn: the commands its turns
//! started and the turns of its subagents. Interrupting a turn ends neither.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::super::json::text_or_empty as text;
use super::notifications::Place;

#[derive(Serialize, Deserialize)]
struct Command {
    thread: String,
    turn: String,
    process: String,
}

#[derive(Default, Serialize, Deserialize)]
pub(super) struct Running {
    commands: HashMap<String, Command>,
    /// The turn each subagent's thread is in.
    subagents: HashMap<String, String>,
}

/// What ends with an interrupted turn.
pub(super) enum Left {
    Command(Value),
    /// A subagent's turn, and the request that interrupts it.
    Subagent(String, Value),
}

impl Running {
    pub fn notice(&mut self, root: &str, method: &str, place: &Place, params: &Value) {
        let (Some(thread), item) = (place.thread_id, &params["item"]) else {
            return;
        };
        match method {
            "item/started" if item["type"] == "commandExecution" => {
                if let (Some(turn), Some(process)) = (place.turn_id, item["processId"].as_str()) {
                    let command = Command {
                        thread: thread.to_owned(),
                        turn: turn.to_owned(),
                        process: process.to_owned(),
                    };
                    self.commands.insert(text(item, "id").to_owned(), command);
                }
            }
            "item/completed" => {
                self.commands.remove(text(item, "id"));
            }
            "turn/started" if thread != root => {
                let turn = text(&params["turn"], "id");
                self.subagents.insert(thread.to_owned(), turn.to_owned());
            }
            "turn/completed" => {
                self.subagents.remove(thread);
            }
            _ => {}
        }
    }

    /// Codex interrupted `turn`, the chat's own or a subagent's.
    pub fn interrupted(&mut self, turn: &str) -> Vec<Left> {
        let commands = self
            .commands
            .values()
            .filter(|command| command.turn == turn)
            .map(|command| {
                Left::Command(json!({ "threadId": command.thread, "processId": command.process }))
            });
        let mut left: Vec<Left> = commands.collect();
        if !self.subagents.values().any(|known| known == turn) {
            left.extend(self.subagents.iter().map(|(thread, turn)| {
                let request = json!({ "threadId": thread, "turnId": turn });
                Left::Subagent(turn.clone(), request)
            }));
        }
        left
    }
}
