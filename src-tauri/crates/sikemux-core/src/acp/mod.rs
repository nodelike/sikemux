//! Chat agents spoken to over the Agent Client Protocol: what the core sends
//! them and how it reads what they send back. The connections themselves run
//! in the server.

pub mod account;
pub mod air;
pub mod native;

use std::path::PathBuf;

use agent_client_protocol::schema::v1::{
    ContentBlock, EmbeddedResource, EmbeddedResourceResource, ResourceLink, TextResourceContents,
};
use serde::Serialize;
use serde_json::{json, Value};
use url::Url;

pub use agent_client_protocol::schema;

use crate::protocol::ChatContext;

const MAX_PROMPT_BYTES: usize = 2 * 1024 * 1024;
const MAX_ATTACHMENTS: usize = 32;
const MAX_CONTEXT_ITEMS: usize = 16;

pub fn bounded_text(name: &str, value: &str, max: usize) -> Result<(), String> {
    if value.is_empty() || value.len() > max || value.contains(['\0', '\r', '\n']) {
        return Err(format!("{name} must be bounded non-blank text"));
    }
    Ok(())
}

/// Why a session ended, so the chat can tell a stop it asked for from an agent
/// that died under it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionEnd {
    Requested,
    Exited,
    Failed,
}

pub fn ended_status(result: &Result<SessionEnd, String>, started: bool) -> Value {
    match result {
        Ok(end) => json!({ "state": "stopped", "reason": end }),
        Err(message) => json!({
            "state": "error",
            "reason": if started { SessionEnd::Exited } else { SessionEnd::Failed },
            "message": message,
        }),
    }
}

/// The session mode that carries a permission mode. Agents whose modes are not
/// about permissions get none, and Sikemux answers their requests itself.
pub fn permission_mode_id(
    provider: &str,
    mode: &str,
    setup: &Value,
) -> Result<Option<&'static str>, String> {
    let expected = match (provider, mode) {
        ("hermes", "bypass") => "dont_ask",
        ("hermes", "workspace-write") => "accept_edits",
        (_, "bypass" | "workspace-write") if native::arguments(provider).is_some() => {
            return Ok(None)
        }
        _ => return Err(format!("Unsupported permission mode: {mode}")),
    };
    setup
        .pointer("/modes/availableModes")
        .and_then(Value::as_array)
        .and_then(|modes| {
            modes
                .iter()
                .filter_map(|mode| mode.get("id").and_then(Value::as_str))
                .find(|id| *id == expected)
        })
        .map(|_| Some(expected))
        .ok_or_else(|| format!("The {provider} adapter does not offer permission mode {expected}"))
}

/// Every agent still asks before some actions in its most open mode, such as
/// Claude Code for an `rm -rf` of a computed path, and some agents have no
/// such mode at all, so under YOLO the host says yes for the person.
pub fn approves_for_user(mode: &str) -> bool {
    mode == "bypass"
}

/// The value a select option currently holds.
pub fn current_choice(setup: &Value, config_id: &str) -> Option<String> {
    setup
        .get("configOptions")?
        .as_array()?
        .iter()
        .find(|option| option["id"] == config_id)?
        .get("currentValue")?
        .as_str()
        .map(str::to_owned)
}

fn resource_link(path: &str) -> Result<ContentBlock, String> {
    bounded_text("attachment path", path, 4_096)?;
    let path = PathBuf::from(path);
    if !path.is_absolute() {
        return Err("attachment paths must be absolute".into());
    }
    let uri = Url::from_file_path(&path)
        .map_err(|_| "attachment path cannot be represented as a file URL")?
        .to_string();
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("attachment")
        .to_owned();
    Ok(ContentBlock::ResourceLink(ResourceLink::new(name, uri)))
}

/// A context item written into the message itself, for an agent that cannot
/// take it as an embedded resource. The fence outgrows any run of backticks
/// in the text so the text cannot close it early.
fn context_section(item: &ChatContext) -> String {
    let longest = item
        .text
        .split(|character| character != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    format!(
        "### {}\n{}\n\n{fence}\n{}\n{fence}",
        item.title, item.uri, item.text
    )
}

fn context_resource(item: ChatContext) -> ContentBlock {
    ContentBlock::Resource(EmbeddedResource::new(
        EmbeddedResourceResource::TextResourceContents(
            TextResourceContents::new(item.text, item.uri).mime_type("text/markdown".to_string()),
        ),
    ))
}

pub fn prompt_blocks(
    mut text: String,
    paths: Vec<String>,
    context: Vec<ChatContext>,
    embedded_context: bool,
) -> Result<Vec<ContentBlock>, String> {
    if paths.len() > MAX_ATTACHMENTS {
        return Err(format!(
            "a prompt can include at most {MAX_ATTACHMENTS} attachments"
        ));
    }
    if context.len() > MAX_CONTEXT_ITEMS {
        return Err(format!(
            "a prompt can include at most {MAX_CONTEXT_ITEMS} context items"
        ));
    }
    for item in &context {
        bounded_text("context title", &item.title, 1_024)?;
        bounded_text("context uri", &item.uri, 4_096)?;
    }
    let context_bytes: usize = context.iter().map(|item| item.text.len()).sum();
    if text.len() + context_bytes > MAX_PROMPT_BYTES {
        return Err("prompt is too large".into());
    }
    let mut resources = Vec::new();
    if embedded_context {
        resources.extend(context.into_iter().map(context_resource));
    } else {
        for item in &context {
            if !text.trim().is_empty() {
                text.push_str("\n\n");
            }
            text.push_str(&context_section(item));
        }
    }
    let mut blocks =
        Vec::with_capacity(paths.len() + resources.len() + usize::from(!text.trim().is_empty()));
    if !text.trim().is_empty() {
        blocks.push(text.into());
    }
    blocks.extend(resources);
    for path in paths {
        blocks.push(resource_link(&path)?);
    }
    if blocks.is_empty() {
        return Err("prompt is empty".into());
    }
    Ok(blocks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permissions_use_advertised_provider_modes() {
        let setup = json!({ "modes": { "availableModes": [{ "id": "accept_edits" }, { "id": "dont_ask" }] } });
        assert_eq!(
            permission_mode_id("hermes", "workspace-write", &setup).unwrap(),
            Some("accept_edits")
        );
        assert_eq!(
            permission_mode_id("hermes", "bypass", &setup).unwrap(),
            Some("dont_ask")
        );
        assert!(permission_mode_id("hermes", "invalid", &setup).is_err());
        assert!(permission_mode_id("hermes", "bypass", &json!({})).is_err());
    }

    #[test]
    fn agents_without_permission_modes_are_answered_by_the_host() {
        for provider in ["opencode", "omp", "grok"] {
            assert_eq!(
                permission_mode_id(provider, "bypass", &json!({})).unwrap(),
                None
            );
            assert_eq!(
                permission_mode_id(provider, "workspace-write", &json!({})).unwrap(),
                None
            );
        }
    }

    #[test]
    fn a_session_says_why_it_ended() {
        assert_eq!(
            ended_status(&Ok(SessionEnd::Requested), true),
            json!({ "state": "stopped", "reason": "requested" })
        );
        assert_eq!(
            ended_status(&Ok(SessionEnd::Exited), true),
            json!({ "state": "stopped", "reason": "exited" })
        );
        assert_eq!(
            ended_status(&Err("Process exited with 1".into()), true),
            json!({ "state": "error", "reason": "exited", "message": "Process exited with 1" })
        );
        assert_eq!(
            ended_status(&Err("no adapter".into()), false),
            json!({ "state": "error", "reason": "failed", "message": "no adapter" })
        );
    }

    #[test]
    fn yolo_answers_every_agent_for_the_person_and_safe_mode_asks_them() {
        assert!(approves_for_user("bypass"));
        assert!(!approves_for_user("workspace-write"));
    }

    #[test]
    fn the_current_choice_is_read_from_the_config_options() {
        let setup = json!({ "configOptions": [
            { "id": "model", "currentValue": "fast" },
            { "id": "effort", "currentValue": "high" },
        ] });
        assert_eq!(current_choice(&setup, "model").as_deref(), Some("fast"));
        assert_eq!(current_choice(&setup, "effort").as_deref(), Some("high"));
        assert_eq!(current_choice(&setup, "thinking"), None);
        assert_eq!(current_choice(&json!({}), "model"), None);
    }

    #[test]
    fn prompt_rejects_relative_attachment_paths() {
        let error = prompt_blocks(String::new(), vec!["relative.txt".into()], Vec::new(), true)
            .unwrap_err();
        assert_eq!(error, "attachment paths must be absolute");
    }

    fn issue_context() -> ChatContext {
        ChatContext {
            uri: "https://github.com/o/r/issues/12".into(),
            title: "#12 Login crashes".into(),
            text: "Issue #12: Login crashes\n\nIt crashes.".into(),
        }
    }

    #[test]
    fn prompt_embeds_context_when_the_agent_takes_it() {
        let blocks = prompt_blocks(
            "fix this".into(),
            vec!["/tmp/a.txt".into()],
            vec![issue_context()],
            true,
        )
        .unwrap();
        let blocks = serde_json::to_value(blocks).unwrap();
        assert_eq!(blocks[0], json!({ "type": "text", "text": "fix this" }));
        assert_eq!(
            blocks[1],
            json!({
                "type": "resource",
                "resource": {
                    "uri": "https://github.com/o/r/issues/12",
                    "mimeType": "text/markdown",
                    "text": "Issue #12: Login crashes\n\nIt crashes.",
                },
            })
        );
        assert_eq!(blocks[2]["type"], "resource_link");
    }

    #[test]
    fn prompt_writes_context_into_the_text_otherwise() {
        let blocks =
            prompt_blocks("fix this".into(), Vec::new(), vec![issue_context()], false).unwrap();
        let blocks = serde_json::to_value(blocks).unwrap();
        assert_eq!(blocks.as_array().unwrap().len(), 1);
        assert_eq!(
            blocks[0]["text"],
            "fix this\n\n### #12 Login crashes\nhttps://github.com/o/r/issues/12\n\n```\nIssue #12: Login crashes\n\nIt crashes.\n```"
        );
    }

    #[test]
    fn prompt_of_context_alone_is_not_empty() {
        let blocks =
            prompt_blocks(String::new(), Vec::new(), vec![issue_context()], false).unwrap();
        assert!(serde_json::to_value(blocks).unwrap()[0]["text"]
            .as_str()
            .unwrap()
            .starts_with("### #12"));
        assert!(prompt_blocks(String::new(), Vec::new(), vec![issue_context()], true).is_ok());
    }

    #[test]
    fn context_fence_outgrows_backticks_in_the_text() {
        let item = ChatContext {
            text: "```rust\nfn main() {}\n```".into(),
            ..issue_context()
        };
        let section = context_section(&item);
        assert!(section.contains("\n````\n```rust"));
        assert!(section.ends_with("\n```\n````"));
    }
}
