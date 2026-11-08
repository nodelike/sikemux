//! The slash commands the chat answers for Pi, which leaves its own built-in
//! commands to its terminal, and the list of commands the chat offers. Skills,
//! prompt templates and extension commands go to Pi as they are written.

use serde_json::Value;

use crate::server::chat::prompt::command_parts;

use super::super::commands::offered as command;

#[derive(Debug, PartialEq)]
pub(super) enum Command {
    Compact(Option<String>),
    Name(String),
    Status,
    Hint(String),
}

pub(super) fn parse(text: &str) -> Option<Command> {
    let (name, rest) = command_parts(text)?;
    match name.to_lowercase().as_str() {
        "compact" => Some(Command::Compact(
            (!rest.is_empty()).then(|| rest.to_owned()),
        )),
        "name" | "rename" if rest.is_empty() => Some(Command::Hint(format!(
            "Command \"/{name}\" requires a name."
        ))),
        "name" | "rename" => Some(Command::Name(rest.to_owned())),
        "status" => Some(Command::Status),
        _ => None,
    }
}

/// The commands the chat offers: the ones handled here, then what Pi's
/// `get_commands` listed, leaving out what Sikemux's own extension registers.
pub(super) fn available(listed: &Value) -> Value {
    let mut commands = vec![
        command(
            "compact",
            "Summarize the conversation to free up context.",
            Some("optional instructions"),
        ),
        command("name", "Name the current session.", Some("name")),
        command(
            "status",
            "Show the session's model, settings and token usage.",
            None,
        ),
    ];
    for listed in listed["commands"].as_array().into_iter().flatten() {
        let Some(name) = listed["name"].as_str().filter(|name| !name.is_empty()) else {
            continue;
        };
        if name.starts_with("sikemux-") || commands.iter().any(|known| known["name"] == name) {
            continue;
        }
        let description = listed["description"].as_str().unwrap_or(name);
        let description = description.lines().next().unwrap_or(description);
        commands.push(command(name, description, Some("optional instructions")));
    }
    Value::Array(commands)
}
