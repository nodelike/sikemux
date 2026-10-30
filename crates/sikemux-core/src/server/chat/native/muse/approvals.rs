//! Muse asking before it acts, as the ACP permission requests the chat shows,
//! and the person's choice back as the decision Muse takes.

use serde_json::{json, Value};

use sikemux_wire::chat::PermissionKind;

use super::super::json::{text, text_or_empty};
use crate::server::chat::permission::{allow_first, for_session, option};

/// One stage of an approval the person is asked about, and how to tell Muse
/// what they chose.
#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct Question {
    pub request: Value,
    approval_id: String,
    requirement: Value,
    /// The choice ids the person picks from, and the one that refuses.
    choices: Vec<String>,
    refusal: Option<String>,
}

impl Question {
    /// Which stage of which approval this asks about, so a stage told twice
    /// is asked once.
    pub fn key(&self) -> (String, String) {
        (self.approval_id.clone(), self.requirement.to_string())
    }

    /// The `approval/decide` params for the option the person chose, or for no
    /// choice at all.
    pub fn decision(
        &self,
        session_id: &str,
        command_id: String,
        chosen: Option<&str>,
    ) -> Option<Value> {
        let choice = chosen
            .filter(|chosen| self.choices.iter().any(|choice| choice == chosen))
            .map(str::to_owned)
            .or_else(|| self.refusal.clone())?;
        Some(json!({
            "sessionId": session_id,
            "commandId": command_id,
            "approvalId": self.approval_id,
            "requirementId": self.requirement,
            "choiceId": choice,
        }))
    }
}

/// The ACP option kind a Muse decision reads as.
fn option_kind(decision: &str) -> Option<PermissionKind> {
    match decision {
        "approved" => Some(PermissionKind::AllowOnce),
        "approvedForSession" | "approvedPolicyAmendment" => Some(PermissionKind::AllowAlways),
        "denied" | "deniedPolicyAmendment" | "abort" => Some(PermissionKind::RejectOnce),
        _ => None,
    }
}

fn title(params: &Value) -> String {
    let subject = &params["subject"];
    let tool = text_or_empty(params, "toolName");
    match text_or_empty(subject, "kind") {
        "shell" | "process" => text(subject, "command")
            .map(str::to_owned)
            .unwrap_or_else(|| tool.to_owned()),
        "fileAccess" => {
            let path = text_or_empty(subject, "path");
            match text(subject, "access") {
                Some("write") => format!("Write {path}"),
                Some("read") => format!("Read {path}"),
                Some(access) => format!("{access} {path}"),
                None => path.to_owned(),
            }
        }
        "network" => {
            let host = text_or_empty(subject, "host");
            match subject["port"].as_u64() {
                Some(port) => format!("Connect to {host}:{port}"),
                None => format!("Connect to {host}"),
            }
        }
        _ => text(subject, "toolName").unwrap_or(tool).to_owned(),
    }
}

fn tool_kind(subject: &Value) -> &'static str {
    match (text_or_empty(subject, "kind"), text(subject, "access")) {
        ("shell" | "process", _) => "execute",
        ("fileAccess", Some("read")) => "read",
        ("fileAccess", _) => "edit",
        ("network", _) => "fetch",
        _ => "other",
    }
}

/// The question for one stage of an approval, asked in `session`. None when
/// Muse offers nothing the chat can show.
pub(super) fn question(session: &str, params: &Value) -> Option<Question> {
    let approval_id = text(params, "approvalId")?.to_owned();
    let requirement = params["currentRequirementId"].clone();
    let subject = &params["subject"];
    let mut options = Vec::new();
    let mut choices = Vec::new();
    let mut refusal = None;
    for choice in params["availableChoices"].as_array().into_iter().flatten() {
        let (Some(id), Some(kind)) = (
            text(choice, "choiceId"),
            text(choice, "decision").and_then(option_kind),
        ) else {
            continue;
        };
        let mut option = option(id, text(choice, "label").unwrap_or(id), kind);
        if choice["scope"] == "session" {
            option = for_session(option);
        }
        if kind == PermissionKind::RejectOnce
            && (refusal.is_none() || choice["decision"] == "abort")
        {
            refusal = Some(id.to_owned());
        }
        choices.push(id.to_owned());
        options.push(option);
    }
    options.sort_by_key(allow_first);
    if options.is_empty() {
        return None;
    }
    let mut raw_input = json!({ "tool": params["toolName"] });
    if let Some(command) = text(subject, "command") {
        raw_input["command"] = json!(command);
    }
    let mut tool_call = json!({
        "toolCallId": params["itemId"],
        "title": title(params),
        "kind": tool_kind(subject),
        "status": "pending",
        "rawInput": raw_input,
    });
    if let Some(path) = text(subject, "path") {
        tool_call["locations"] = json!([{ "path": path }]);
    }
    let mut permission = json!({ "version": 1, "title": title(params) });
    let reasons: Vec<&str> = [
        (params["judgeEscalated"] == true).then_some("Muse's reviewer wants you to decide"),
        (params["protectedWrite"] == true).then_some("This writes to a protected path"),
    ]
    .into_iter()
    .flatten()
    .collect();
    if !reasons.is_empty() {
        permission["description"] = json!(reasons.join(". "));
    }
    Some(Question {
        request: json!({
            "sessionId": session,
            "toolCall": tool_call,
            "options": options,
            "_meta": { "permission": permission },
        }),
        approval_id,
        requirement,
        choices,
        refusal,
    })
}
