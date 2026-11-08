//! What Pi's extensions ask the person, as the permission questions the chat
//! shows, and the person's choice back as Pi's response. The tool hook Sikemux
//! adds asks with the title [`TOOL_PERMISSION`], so its question reads as the
//! tool call it is about.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use sikemux_view::chat::PermissionKind;
use sikemux_view::questions::QuestionChoice;

use super::super::json::text;
use super::tools;
use crate::server::chat::permission::{for_session, option, request};

const TOOL_PERMISSION: &str = "sikemux-tool-permission";

const ALLOW: &str = "allow";
const ALWAYS: &str = "always";
const REJECT: &str = "reject";
const CANCEL: &str = "cancel";

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub(crate) enum Answer {
    Tool { remember: String },
    Confirm,
    Select { options: Vec<String> },
}

/// One of Pi's dialogs, kept to answer it.
#[derive(Serialize, Deserialize)]
pub(crate) struct Asked {
    pub id: String,
    pub answer: Answer,
}

pub(super) enum Dialog {
    /// The tool hook asked about a tool the person already allowed for the session.
    Allowed(String),
    /// A question about a tool, which YOLO answers for the person.
    Permission(Value, Asked),
    /// A question only the person can answer.
    Question(Value, Asked),
    /// A question the person answers in words of their own.
    Words(Words),
}

/// An `input` or `editor` dialog, put to the person as a question they type
/// an answer to. Its first choice answers with the text it opened with.
pub(super) struct Words {
    pub id: String,
    pub title: String,
    pub text: String,
    pub prefill: Option<String>,
}

impl Words {
    pub fn choices(&self) -> Vec<QuestionChoice> {
        let choice = |label: &str| QuestionChoice {
            label: label.to_owned(),
            detail: None,
            recommended: false,
        };
        let keep = if self.prefill.is_some() {
            "Keep this text"
        } else {
            "Leave it empty"
        };
        vec![choice(keep), choice("Cancel")]
    }
}

/// Pi's response to a dialog the person answered in words, or by the choice
/// at `choice`.
pub(super) fn words_response(
    id: &str,
    prefill: Option<String>,
    answer: String,
    choice: Option<usize>,
) -> Value {
    let mut record = json!({ "type": "extension_ui_response", "id": id });
    match choice {
        Some(0) => record["value"] = json!(prefill.unwrap_or_default()),
        Some(_) => record["cancelled"] = json!(true),
        None => record["value"] = json!(answer),
    }
    record
}

fn visible<'a>(event: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| text(event, key))
        .filter(|said| !said.trim().is_empty())
}

fn card(session: &str, id: &str, title: &str, detail: Option<&str>, options: &[Value]) -> Value {
    let mut tool_call = json!({
        "toolCallId": format!("pi-dialog-{id}"),
        "title": title,
        "kind": "other",
        "status": "pending",
    });
    if let Some(detail) = detail {
        tool_call["content"] = json!([sikemux_fold::update::text_content(detail)]);
    }
    request(session, tool_call, options)
}

fn tool_permission(
    session: &str,
    id: &str,
    cwd: &str,
    message: &str,
) -> Option<(String, Value, Asked)> {
    let asked: Value = serde_json::from_str(message).ok()?;
    let tool = text(&asked, "toolName")?;
    let input = &asked["input"];
    let call_id = text(&asked, "toolCallId").unwrap_or(id);
    let remember = if tools::is_bash(tool) {
        format!("{tool}:{}", text(input, "command").unwrap_or_default())
    } else {
        tool.to_owned()
    };
    let call = tools::call(call_id, tool, input, cwd, "pending");
    let options = [
        option(ALLOW, "Allow", PermissionKind::AllowOnce),
        for_session(option(
            ALWAYS,
            if tools::is_bash(tool) {
                "Always allow this command"
            } else {
                "Always allow this tool"
            },
            PermissionKind::AllowAlways,
        )),
        option(REJECT, "Reject", PermissionKind::RejectOnce),
    ];
    Some((
        remember.clone(),
        request(session, call, &options),
        Asked {
            id: id.to_owned(),
            answer: Answer::Tool { remember },
        },
    ))
}

/// The question a dialog puts to the person, when it is one that waits on them.
pub(super) fn read(
    session: &str,
    cwd: &str,
    event: &Value,
    allowed: impl Fn(&str) -> bool,
) -> Option<Dialog> {
    let id = text(event, "id")?;
    let method = text(event, "method")?;
    let title = text(event, "title").unwrap_or(method);
    let detail = visible(event, &["message", "placeholder"]);
    let asked = |answer| Asked {
        id: id.to_owned(),
        answer,
    };
    match method {
        "confirm" if title == TOOL_PERMISSION => {
            let (remember, request, asked) =
                tool_permission(session, id, cwd, text(event, "message")?)?;
            Some(if allowed(&remember) {
                Dialog::Allowed(id.to_owned())
            } else {
                Dialog::Permission(request, asked)
            })
        }
        "confirm" => Some(Dialog::Question(
            card(
                session,
                id,
                title,
                detail,
                &[
                    option(ALLOW, "Yes", PermissionKind::AllowOnce),
                    option(REJECT, "No", PermissionKind::RejectOnce),
                ],
            ),
            asked(Answer::Confirm),
        )),
        "select" => {
            let options: Vec<String> = event["options"]
                .as_array()?
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
            let mut shown: Vec<Value> = options
                .iter()
                .enumerate()
                .map(|(at, label)| {
                    let label = if label.is_empty() {
                        "Empty value"
                    } else {
                        label
                    };
                    option(&at.to_string(), label, PermissionKind::AllowOnce)
                })
                .collect();
            shown.push(option(CANCEL, "Cancel", PermissionKind::RejectOnce));
            Some(Dialog::Question(
                card(session, id, title, detail, &shown),
                asked(Answer::Select { options }),
            ))
        }
        "input" | "editor" => {
            let prefill = text(event, "prefill").map(str::to_owned);
            let text = match prefill.as_deref().or(detail) {
                Some(body) => format!("{title}\n\n{body}"),
                None => title.to_owned(),
            };
            Some(Dialog::Words(Words {
                id: id.to_owned(),
                title: title.to_owned(),
                text,
                prefill,
            }))
        }
        _ => None,
    }
}

/// Pi's response to the dialog, and what the session should remember allowing.
pub(super) fn response(asked: Asked, chosen: Option<&str>) -> (Value, Option<String>) {
    let mut record = json!({ "type": "extension_ui_response", "id": asked.id });
    let mut remember = None;
    match (asked.answer, chosen) {
        (Answer::Tool { remember: tool }, Some(ALWAYS)) => {
            remember = Some(tool);
            record["confirmed"] = json!(true);
        }
        (Answer::Tool { .. } | Answer::Confirm, Some(ALLOW)) => record["confirmed"] = json!(true),
        (Answer::Tool { .. } | Answer::Confirm, Some(REJECT)) => record["confirmed"] = json!(false),
        (Answer::Select { options }, Some(chosen)) => {
            match chosen.parse::<usize>().ok().and_then(|at| options.get(at)) {
                Some(value) => record["value"] = json!(value),
                None => record["cancelled"] = json!(true),
            }
        }
        _ => record["cancelled"] = json!(true),
    }
    (record, remember)
}
