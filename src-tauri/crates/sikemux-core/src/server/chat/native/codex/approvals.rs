//! Codex asking before it acts, as the ACP permission requests its adapter
//! sends, and the person's choice back as the answer Codex expects.

use serde_json::{json, Value};

use super::items::strip_shell;

fn option(id: &str, name: &str, kind: &str) -> Value {
    json!({ "optionId": id, "name": name, "kind": kind })
}

fn request(
    session_id: &str,
    tool_call: Value,
    options: &[Value],
    title: &str,
    reason: &Value,
) -> Value {
    let mut permission = json!({ "version": 1, "title": title });
    if let Some(reason) = reason
        .as_str()
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
    {
        permission["description"] = json!(reason);
    }
    json!({
        "sessionId": session_id,
        "toolCall": tool_call,
        "options": options,
        "_meta": { "permission": permission },
    })
}

fn text_content(text: &str) -> Value {
    json!({ "type": "content", "content": { "type": "text", "text": text } })
}

fn unique(paths: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = Vec::new();
    for path in paths {
        if !seen.contains(&path) {
            seen.push(path);
        }
    }
    seen
}

fn with_locations(tool_call: &mut Value, paths: Vec<String>) {
    if !paths.is_empty() {
        tool_call["locations"] = paths
            .into_iter()
            .map(|path| json!({ "path": path }))
            .collect();
    }
}

/// A question for the person, and how to tell Codex what they chose.
pub(super) struct Question {
    pub request: Value,
    answer: Answer,
    /// The tool call to mark running again once the person allows it.
    pub resumes: Option<String>,
}

/// An option's id, and the decision Codex is told when it is chosen.
type Decision = (String, Value);

enum Answer {
    /// Each option stands for one decision.
    Decisions(Vec<Decision>, Value),
    Permissions(Value),
    Elicitation {
        tool: bool,
        session: bool,
        always: bool,
    },
}

impl Question {
    /// Codex's answer for the option the person chose, or for no choice.
    pub fn answer(&self, chosen: Option<&str>) -> Value {
        match &self.answer {
            Answer::Decisions(decisions, refused) => {
                let decision = chosen
                    .and_then(|chosen| decisions.iter().find(|(id, _)| id == chosen))
                    .map_or_else(|| refused.clone(), |(_, decision)| decision.clone());
                json!({ "decision": decision })
            }
            Answer::Permissions(requested) => {
                let granted = |scope: &str, strict: bool| {
                    let mut permissions = json!({});
                    for key in ["network", "fileSystem"] {
                        if !requested[key].is_null() {
                            permissions[key] = requested[key].clone();
                        }
                    }
                    json!({ "permissions": permissions, "scope": scope, "strictAutoReview": strict })
                };
                match chosen {
                    Some("allow_permissions_turn") => granted("turn", false),
                    Some("allow_permissions_turn_strict_auto_review") => granted("turn", true),
                    Some("allow_permissions_session") => granted("session", false),
                    _ => refused_permissions(),
                }
            }
            Answer::Elicitation {
                tool,
                session,
                always,
            } => {
                let accept =
                    |meta: Value| json!({ "action": "accept", "content": null, "_meta": meta });
                match chosen {
                    Some("allow_session") if *session => accept(json!({ "persist": "session" })),
                    Some("allow_always") if *always => accept(json!({ "persist": "always" })),
                    Some("allow_once") if *tool => accept(Value::Null),
                    Some("accept") if !*tool => accept(Value::Null),
                    Some("decline") if !*tool => {
                        json!({ "action": "decline", "content": null, "_meta": null })
                    }
                    _ => cancelled_elicitation(),
                }
            }
        }
    }

    pub fn allows(&self, chosen: Option<&str>) -> bool {
        self.answer(chosen)["action"] == "accept"
    }
}

pub(super) fn refused_permissions() -> Value {
    json!({ "permissions": {}, "scope": "turn", "strictAutoReview": false })
}

pub(super) fn cancelled_elicitation() -> Value {
    json!({ "action": "cancel", "content": null, "_meta": null })
}

fn decision_options(decisions: &[(Value, Value)]) -> Option<(Vec<Value>, Vec<Decision>)> {
    let mut ordered: Vec<&(Value, Value)> = decisions.iter().collect();
    let rank = |option: &Value| match option["kind"].as_str() {
        Some("allow_once") => 0,
        Some("allow_always") => 1,
        _ => 2,
    };
    ordered.sort_by_key(|(option, _)| rank(option));
    let kinds: Vec<&str> = ordered
        .iter()
        .filter_map(|(option, _)| option["kind"].as_str())
        .collect();
    let allows = kinds.iter().any(|kind| kind.starts_with("allow"));
    let rejects = kinds.iter().any(|kind| kind.starts_with("reject"));
    let ids: Vec<String> = ordered
        .iter()
        .filter_map(|(option, _)| option["optionId"].as_str().map(str::to_owned))
        .collect();
    let distinct = unique(ids.clone()).len() == ids.len();
    if !(allows && rejects && distinct) {
        return None;
    }
    let options = ordered.iter().map(|(option, _)| option.clone()).collect();
    let answers = ids
        .into_iter()
        .zip(ordered.iter().map(|(_, decision)| decision.clone()))
        .collect();
    Some((options, answers))
}

/// The decisions Codex offers for a command, or its usual ones when it names
/// none. None when what it offers cannot be shown faithfully.
fn command_decisions(params: &Value) -> Option<Vec<Value>> {
    let network = !params["networkApprovalContext"].is_null();
    let proposed = &params["proposedExecpolicyAmendment"];
    let Some(available) = params["availableDecisions"].as_array() else {
        let mut decisions = vec![json!("accept"), json!("acceptForSession")];
        if network {
            for amendment in params["proposedNetworkPolicyAmendments"]
                .as_array()
                .into_iter()
                .flatten()
            {
                decisions.push(json!({ "applyNetworkPolicyAmendment": { "network_policy_amendment": amendment } }));
            }
        } else if !params["additionalPermissions"].is_null() {
            return Some(vec![json!("accept"), json!("cancel")]);
        } else if proposed.as_array().is_some_and(|words| !words.is_empty()) {
            decisions.push(
                json!({ "acceptWithExecpolicyAmendment": { "execpolicy_amendment": proposed } }),
            );
        }
        decisions.extend([json!("decline"), json!("cancel")]);
        return Some(decisions);
    };
    if available.is_empty() {
        return None;
    }
    available
        .iter()
        .map(|decision| match decision {
            Value::String(name) => matches!(
                name.as_str(),
                "accept" | "acceptForSession" | "decline" | "cancel"
            )
            .then(|| decision.clone()),
            Value::Object(_) => {
                if let Some(amendment) =
                    decision.pointer("/acceptWithExecpolicyAmendment/execpolicy_amendment")
                {
                    let usable = amendment.as_array().is_some_and(|words| {
                        !words.is_empty() && words.iter().all(Value::is_string)
                    }) && amendment == proposed;
                    return usable.then(|| decision.clone());
                }
                let amendment =
                    decision.pointer("/applyNetworkPolicyAmendment/network_policy_amendment")?;
                let host = params["networkApprovalContext"]["host"].as_str()?;
                let proposed = params["proposedNetworkPolicyAmendments"].as_array()?;
                let usable = amendment["host"] == host
                    && matches!(amendment["action"].as_str(), Some("allow" | "deny"))
                    && proposed.iter().any(|candidate| {
                        candidate["host"] == amendment["host"]
                            && candidate["action"] == amendment["action"]
                    });
                usable.then(|| decision.clone())
            }
            _ => None,
        })
        .collect()
}

/// A command prefix as the person would type it.
fn rendered_prefix(words: &[Value]) -> String {
    let words: Vec<&str> = words.iter().filter_map(Value::as_str).collect();
    let shell = |word: &str| {
        let name = word.rsplit('/').next().unwrap_or(word);
        matches!(name, "bash" | "zsh" | "sh")
    };
    if let [program, flag, script] = words.as_slice() {
        if shell(program) && matches!(*flag, "-lc" | "-c") {
            return (*script).to_owned();
        }
    }
    words
        .iter()
        .map(|word| {
            let plain = !word.is_empty()
                && word.chars().all(|character| {
                    character.is_ascii_alphanumeric() || "+-./:@]_".contains(character)
                });
            if plain {
                (*word).to_owned()
            } else {
                format!("'{}'", word.replace('\'', "'\\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn command_options(params: &Value) -> Option<Vec<(Value, Value)>> {
    let network = !params["networkApprovalContext"].is_null();
    let permissions = !params["additionalPermissions"].is_null();
    let mut amendments = 0;
    let mut options = Vec::new();
    for decision in command_decisions(params)? {
        let option = match decision.as_str() {
            Some("accept") => option(
                "allow_once",
                if network {
                    "Yes, just this once"
                } else {
                    "Yes, proceed"
                },
                "allow_once",
            ),
            Some("acceptForSession") => option(
                "allow_for_session",
                if network {
                    "Yes, and allow this host for this conversation"
                } else if permissions {
                    "Yes, and allow these permissions for this session"
                } else {
                    "Yes, and don't ask again for this command in this session"
                },
                "allow_always",
            ),
            Some("decline") => option("decline", "No, continue without running it", "reject_once"),
            Some("cancel") => option(
                "cancel",
                "No, and tell Codex what to do differently",
                "reject_once",
            ),
            _ => {
                if let Some(words) = decision
                    .pointer("/acceptWithExecpolicyAmendment/execpolicy_amendment")
                    .and_then(Value::as_array)
                {
                    let prefix = rendered_prefix(words);
                    if prefix.contains(['\n', '\r']) {
                        continue;
                    }
                    option(
                        "accept_execpolicy_amendment",
                        &format!(
                            "Yes, and don't ask again for commands that start with `{prefix}`"
                        ),
                        "allow_always",
                    )
                } else {
                    let allow = decision
                        .pointer("/applyNetworkPolicyAmendment/network_policy_amendment/action")
                        .is_some_and(|action| action == "allow");
                    let id = format!("apply_network_policy_amendment:{amendments}");
                    amendments += 1;
                    if allow {
                        option(
                            &id,
                            "Yes, and allow this host in the future",
                            "allow_always",
                        )
                    } else {
                        option(
                            &id,
                            "No, and block this host in the future",
                            "reject_always",
                        )
                    }
                }
            }
        };
        options.push((option, decision));
    }
    Some(options)
}

fn command_title(actions: &Value) -> &'static str {
    let actions = actions.as_array().map(Vec::as_slice).unwrap_or_default();
    match actions.first().and_then(|action| action["type"].as_str()) {
        None => "Run command",
        Some("read") if actions.len() == 1 => "Read file",
        Some("read") => "Run command with file reads",
        Some("listFiles") => "List files",
        Some("search") => "Search files",
        Some(_) => "Run command",
    }
}

fn permission_paths(permissions: &Value) -> Vec<String> {
    let file_system = &permissions["fileSystem"];
    let listed = ["read", "write"]
        .into_iter()
        .flat_map(|key| file_system[key].as_array().cloned().unwrap_or_default());
    let entries = file_system["entries"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|entry| entry["path"]["type"] == "path")
        .map(|entry| entry["path"]["path"].clone());
    unique(
        listed
            .chain(entries)
            .filter_map(|path| path.as_str().map(str::to_owned)),
    )
}

fn permission_content(permissions: &Value) -> Vec<Value> {
    let mut lines = Vec::new();
    if let Some(enabled) = permissions["network"]["enabled"].as_bool() {
        lines.push(
            if enabled {
                "Enable network access"
            } else {
                "Disable network access"
            }
            .to_owned(),
        );
    }
    for entry in permissions["fileSystem"]["entries"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let access = entry["access"].as_str().unwrap_or_default();
        let path = &entry["path"];
        match path["type"].as_str() {
            Some("glob_pattern") => lines.push(format!(
                "{access} filesystem pattern {}",
                path["pattern"].as_str().unwrap_or_default()
            )),
            Some("special") => {
                lines.push(format!("{access} Codex filesystem scope {}", path["value"]))
            }
            _ => {}
        }
    }
    if lines.is_empty() {
        Vec::new()
    } else {
        vec![text_content(&lines.join("\n"))]
    }
}

/// `item/commandExecution/requestApproval`. None when it cannot be asked
/// faithfully, which Codex is told as a cancel.
pub(super) fn command(session_id: &str, params: &Value) -> Option<Question> {
    let (options, answers) = decision_options(&command_options(params)?)?;
    let network = &params["networkApprovalContext"];
    let mut input = json!({});
    if let Some(command) = params["command"]
        .as_str()
        .filter(|command| !command.is_empty())
    {
        input["command"] = json!(strip_shell(command));
    }
    if let Some(cwd) = params["cwd"].as_str().filter(|cwd| !cwd.is_empty()) {
        input["cwd"] = json!(cwd);
    }
    let protocol = network["protocol"].as_str().unwrap_or_default();
    let host = network["host"].as_str().unwrap_or_default();
    if matches!(protocol, "http" | "https") {
        input["url"] = json!(format!("{protocol}://{host}"));
    }
    if !params["additionalPermissions"].is_null() {
        input["additionalPermissions"] = params["additionalPermissions"].clone();
    }
    let mut tool_call = json!({
        "toolCallId": params["itemId"],
        "kind": "execute",
        "status": "pending",
        "title": if network.is_null() {
            command_title(&params["commandActions"]).to_owned()
        } else {
            format!("{protocol} network access to {host}")
        },
    });
    if input.as_object().is_some_and(|input| !input.is_empty()) {
        tool_call["rawInput"] = input;
    }
    let action_paths = params["commandActions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|action| action["path"].as_str().map(str::to_owned));
    with_locations(
        &mut tool_call,
        unique(action_paths.chain(permission_paths(&params["additionalPermissions"]))),
    );
    let mut content = Vec::new();
    if !network.is_null() {
        content.push(text_content(&format!("{protocol} access to {host}")));
    }
    if !params["additionalPermissions"].is_null() {
        content.extend(permission_content(&params["additionalPermissions"]));
    }
    if !content.is_empty() {
        tool_call["content"] = json!(content);
    }
    let title = if network.is_null() {
        "Run command?"
    } else {
        "Allow network access?"
    };
    Some(Question {
        request: request(session_id, tool_call, &options, title, &params["reason"]),
        answer: Answer::Decisions(answers, json!("cancel")),
        resumes: None,
    })
}

/// `item/fileChange/requestApproval`. The paths come from the edit's item,
/// which Codex sent when it began.
pub(super) fn file_change(session_id: &str, params: &Value, item: Option<&Value>) -> Question {
    let options = [
        option("allow_once", "Yes, proceed", "allow_once"),
        option(
            "allow_for_session",
            "Yes, and don't ask again for these files",
            "allow_always",
        ),
        option(
            "cancel",
            "No, and tell Codex what to do differently",
            "reject_once",
        ),
    ];
    let mut tool_call = json!({
        "toolCallId": params["itemId"],
        "kind": "edit",
        "status": "pending",
        "title": "Edit files",
    });
    let paths = item
        .and_then(|item| item["changes"].as_array())
        .into_iter()
        .flatten()
        .filter_map(|change| change["path"].as_str().map(str::to_owned));
    with_locations(&mut tool_call, unique(paths));
    Question {
        request: request(
            session_id,
            tool_call,
            &options,
            "Make edits?",
            &params["reason"],
        ),
        answer: Answer::Decisions(
            vec![
                ("allow_once".into(), json!("accept")),
                ("allow_for_session".into(), json!("acceptForSession")),
                ("cancel".into(), json!("cancel")),
            ],
            json!("cancel"),
        ),
        resumes: None,
    }
}

/// `item/permissions/requestApproval`.
pub(super) fn permissions(session_id: &str, params: &Value) -> Question {
    let permissions = &params["permissions"];
    let options = [
        option(
            "allow_permissions_turn",
            "Yes, grant these permissions for this turn",
            "allow_once",
        ),
        option(
            "allow_permissions_turn_strict_auto_review",
            "Yes, grant for this turn with strict auto review",
            "allow_once",
        ),
        option(
            "allow_permissions_session",
            "Yes, grant these permissions for this session",
            "allow_always",
        ),
        option(
            "reject_permissions",
            "No, continue without permissions",
            "reject_once",
        ),
    ];
    let mut tool_call = json!({
        "toolCallId": params["itemId"],
        "kind": "other",
        "status": "pending",
        "title": "Additional sandbox permissions",
        "rawInput": {
            "permissions": permissions,
            "cwd": params["cwd"],
            "environmentId": params["environmentId"],
        },
    });
    with_locations(&mut tool_call, permission_paths(permissions));
    let content = permission_content(permissions);
    if !content.is_empty() {
        tool_call["content"] = json!(content);
    }
    Question {
        request: request(
            session_id,
            tool_call,
            &options,
            "Grant permissions?",
            &params["reason"],
        ),
        answer: Answer::Permissions(permissions.clone()),
        resumes: None,
    }
}

/// Whether an elicitation asks for nothing but a yes or no.
fn message_only(params: &Value) -> bool {
    if !matches!(
        params["mode"].as_str(),
        Some("form" | "openai/form" | "openaiForm")
    ) {
        return false;
    }
    let schema = &params["requestedSchema"];
    schema.is_null()
        || (schema["type"] == "object"
            && schema["properties"]
                .as_object()
                .is_some_and(serde_json::Map::is_empty))
}

/// Whether an elicitation is a tool server asking to run one of its tools.
pub(super) fn is_tool_approval(params: &Value) -> bool {
    params["_meta"]["codex_approval_kind"] == "mcp_tool_call" && message_only(params)
}

/// `mcpServer/elicitation/request`. Sikemux has no forms, so only yes or no
/// questions and links are asked; None is answered as a cancel.
/// `correlated` is the tool call the approval is for, when it can be told;
/// `standalone` names the question otherwise.
pub(super) fn elicitation(
    session_id: &str,
    params: &Value,
    correlated: Option<String>,
    standalone: impl FnOnce() -> String,
) -> Option<Question> {
    let url = params["mode"] == "url";
    if !url && !message_only(params) {
        return None;
    }
    let tool = is_tool_approval(params);
    let persist = &params["_meta"]["persist"];
    let persists = |scope: &str| {
        persist == scope
            || persist
                .as_array()
                .is_some_and(|scopes| scopes.iter().any(|item| item == scope))
    };
    let (session, always) = (persists("session"), persists("always"));
    let described = |id: &str, name: &str, kind: &str, description: &str| {
        let mut option = option(id, name, kind);
        option["_meta"] = json!({ "permission": { "version": 1, "description": description } });
        option
    };
    let mut options = vec![if tool {
        described(
            "allow_once",
            "Allow",
            "allow_once",
            "Run the tool and continue.",
        )
    } else {
        described(
            "accept",
            "Allow",
            "allow_once",
            "Allow this request and continue.",
        )
    }];
    if session {
        options.push(described(
            "allow_session",
            "Allow for this session",
            "allow_always",
            if tool {
                "Run the tool and remember this choice for this session."
            } else {
                "Allow this request and remember this choice for this session."
            },
        ));
    }
    if always {
        options.push(described(
            "allow_always",
            "Always allow",
            "allow_always",
            if tool {
                "Run the tool and remember this choice for future tool calls."
            } else {
                "Allow this request and remember this choice for future requests."
            },
        ));
    }
    if tool {
        options.push(described(
            "cancel",
            "Cancel",
            "reject_once",
            "Cancel this tool call",
        ));
    } else {
        options.push(described(
            "decline",
            "Deny",
            "reject_once",
            "Decline this request and continue.",
        ));
        options.push(described(
            "cancel",
            "Cancel",
            "reject_once",
            "Cancel this request",
        ));
    }
    let message = text_content(params["message"].as_str().unwrap_or_default());
    let mut request = json!({ "sessionId": session_id, "options": options });
    let mut resumes = None;
    if url {
        request["toolCall"] = json!({
            "toolCallId": format!("elicitation-{}", params["elicitationId"].as_str().unwrap_or_default()),
            "kind": "fetch",
            "status": "pending",
            "content": [message],
            "rawInput": { "serverName": params["serverName"], "url": params["url"] },
        });
    } else if let Some(call) = correlated {
        request["toolCall"] = json!({ "toolCallId": call, "kind": "execute", "status": "pending" });
        request["_meta"] = json!({ "is_mcp_tool_approval": true });
        resumes = Some(call);
    } else {
        request["toolCall"] = json!({
            "toolCallId": standalone(),
            "kind": if tool { "execute" } else { "other" },
            "status": "pending",
            "content": [message],
            "rawInput": { "serverName": params["serverName"], "schema": params["requestedSchema"] },
        });
        if tool {
            request["_meta"] = json!({ "is_mcp_tool_approval": true });
        }
    }
    Some(Question {
        request,
        answer: Answer::Elicitation {
            tool,
            session,
            always,
        },
        resumes,
    })
}

/// `execCommandApproval`, which older Codex servers send.
pub(super) fn legacy_command(session_id: &str, params: &Value) -> Question {
    let words: Vec<&str> = params["command"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let options = [
        option("allow_once", "Yes, proceed", "allow_once"),
        option(
            "allow_for_session",
            "Yes, and don't ask again for this command in this session",
            "allow_always",
        ),
        option(
            "cancel",
            "No, and tell Codex what to do differently",
            "reject_once",
        ),
    ];
    let tool_call = json!({
        "toolCallId": params["callId"],
        "kind": "execute",
        "status": "pending",
        "title": "Run command",
        "rawInput": { "command": strip_shell(&words.join(" ")), "cwd": params["cwd"] },
    });
    Question {
        request: request(
            session_id,
            tool_call,
            &options,
            "Run command?",
            &params["reason"],
        ),
        answer: Answer::Decisions(
            vec![
                ("allow_once".into(), json!("approved")),
                ("allow_for_session".into(), json!("approved_for_session")),
                ("cancel".into(), json!("abort")),
            ],
            json!("abort"),
        ),
        resumes: None,
    }
}

/// `applyPatchApproval`, which older Codex servers send.
pub(super) fn legacy_patch(session_id: &str, params: &Value) -> Question {
    let options = [
        option("allow_once", "Yes, proceed", "allow_once"),
        option(
            "allow_for_session",
            "Yes, and don't ask again for these files",
            "allow_always",
        ),
        option(
            "cancel",
            "No, and tell Codex what to do differently",
            "reject_once",
        ),
    ];
    let mut tool_call = json!({
        "toolCallId": params["callId"],
        "kind": "edit",
        "status": "pending",
        "title": "Edit files",
    });
    let paths = params["fileChanges"]
        .as_object()
        .map(|changes| changes.keys().cloned().collect())
        .unwrap_or_default();
    with_locations(&mut tool_call, paths);
    Question {
        request: request(
            session_id,
            tool_call,
            &options,
            "Make edits?",
            &params["reason"],
        ),
        answer: Answer::Decisions(
            vec![
                ("allow_once".into(), json!("approved")),
                ("allow_for_session".into(), json!("approved_for_session")),
                ("cancel".into(), json!("abort")),
            ],
            json!("abort"),
        ),
        resumes: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(question: &Question) -> Vec<String> {
        question.request["options"]
            .as_array()
            .unwrap()
            .iter()
            .map(|option| option["optionId"].as_str().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn a_command_offers_the_adapters_choices_in_order() {
        let params = json!({
            "threadId": "t", "turnId": "u", "itemId": "exec-1", "reason": "needs network",
            "command": "/bin/zsh -lc 'git push'", "cwd": "/w",
            "commandActions": [{ "type": "unknown", "command": "git push" }],
            "networkApprovalContext": null, "additionalPermissions": null,
            "proposedExecpolicyAmendment": ["git", "push"], "proposedNetworkPolicyAmendments": null,
            "availableDecisions": null,
        });
        let question = command("t", &params).unwrap();
        assert_eq!(
            ids(&question),
            [
                "allow_once",
                "allow_for_session",
                "accept_execpolicy_amendment",
                "decline",
                "cancel"
            ]
        );
        let request = &question.request;
        assert_eq!(request["sessionId"], "t");
        assert_eq!(request["toolCall"]["toolCallId"], "exec-1");
        assert_eq!(request["toolCall"]["title"], "Run command");
        assert_eq!(
            request["toolCall"]["rawInput"],
            json!({ "command": "git push", "cwd": "/w" })
        );
        assert_eq!(
            request["options"][2]["name"],
            "Yes, and don't ask again for commands that start with `git push`"
        );
        assert_eq!(
            request["_meta"],
            json!({ "permission": { "version": 1, "title": "Run command?", "description": "needs network" } })
        );
        assert_eq!(
            question.answer(Some("allow_once")),
            json!({ "decision": "accept" })
        );
        assert_eq!(
            question.answer(Some("decline")),
            json!({ "decision": "decline" })
        );
        assert_eq!(
            question.answer(Some("accept_execpolicy_amendment")),
            json!({ "decision": { "acceptWithExecpolicyAmendment": { "execpolicy_amendment": ["git", "push"] } } })
        );
        assert_eq!(question.answer(None), json!({ "decision": "cancel" }));
        assert_eq!(
            question.answer(Some("unknown")),
            json!({ "decision": "cancel" })
        );
    }

    #[test]
    fn a_network_request_names_the_host() {
        let params = json!({
            "itemId": "exec-2", "command": "curl x", "cwd": "/w", "commandActions": [],
            "networkApprovalContext": { "host": "example.com", "protocol": "https" },
            "proposedNetworkPolicyAmendments": [{ "host": "example.com", "action": "deny" }],
        });
        let question = command("t", &params).unwrap();
        assert_eq!(
            ids(&question),
            [
                "allow_once",
                "allow_for_session",
                "apply_network_policy_amendment:0",
                "decline",
                "cancel"
            ]
        );
        assert_eq!(
            question.request["toolCall"]["title"],
            "https network access to example.com"
        );
        assert_eq!(
            question.request["toolCall"]["rawInput"]["url"],
            "https://example.com"
        );
        assert_eq!(
            question.request["options"][0]["name"],
            "Yes, just this once"
        );
        assert_eq!(
            question.request["_meta"]["permission"]["title"],
            "Allow network access?"
        );
        assert_eq!(
            question.answer(Some("apply_network_policy_amendment:0")),
            json!({ "decision": { "applyNetworkPolicyAmendment": {
                "network_policy_amendment": { "host": "example.com", "action": "deny" },
            } } })
        );
    }

    #[test]
    fn decisions_codex_names_are_kept_and_unshowable_ones_cancel() {
        let mut params =
            json!({ "itemId": "e", "command": "ls", "availableDecisions": ["accept", "cancel"] });
        assert_eq!(
            ids(&command("t", &params).unwrap()),
            ["allow_once", "cancel"]
        );
        params["availableDecisions"] = json!(["accept"]);
        assert!(command("t", &params).is_none());
        params["availableDecisions"] = json!(["accept", "cancel", "bogus"]);
        assert!(command("t", &params).is_none());
    }

    #[test]
    fn an_edit_lists_its_files_and_has_no_decline() {
        let item =
            json!({ "changes": [{ "path": "/w/a" }, { "path": "/w/b" }, { "path": "/w/a" }] });
        let question = file_change("t", &json!({ "itemId": "exec-3" }), Some(&item));
        assert_eq!(
            ids(&question),
            ["allow_once", "allow_for_session", "cancel"]
        );
        assert_eq!(
            question.request["toolCall"]["locations"],
            json!([{ "path": "/w/a" }, { "path": "/w/b" }])
        );
        assert_eq!(
            question.request["_meta"]["permission"]["title"],
            "Make edits?"
        );
        assert_eq!(
            question.answer(Some("allow_for_session")),
            json!({ "decision": "acceptForSession" })
        );
    }

    #[test]
    fn permissions_are_granted_for_a_scope_or_refused() {
        let params = json!({
            "itemId": "p", "cwd": "/w", "environmentId": null,
            "permissions": { "network": { "enabled": true }, "fileSystem": null },
        });
        let question = permissions("t", &params);
        assert_eq!(
            question.answer(Some("allow_permissions_session")),
            json!({ "permissions": { "network": { "enabled": true } }, "scope": "session", "strictAutoReview": false })
        );
        assert_eq!(question.answer(None), refused_permissions());
        assert_eq!(
            question.request["toolCall"]["content"][0]["content"]["text"],
            "Enable network access"
        );
    }

    #[test]
    fn a_tool_approval_lands_on_its_call_and_forms_are_cancelled() {
        let params = json!({
            "threadId": "t", "serverName": "s", "mode": "form", "message": "Run tool?",
            "requestedSchema": { "type": "object", "properties": {} },
            "_meta": { "codex_approval_kind": "mcp_tool_call", "persist": ["session"] },
        });
        let question = elicitation("t", &params, Some("call_1".into()), || unreachable!()).unwrap();
        assert_eq!(ids(&question), ["allow_once", "allow_session", "cancel"]);
        assert_eq!(question.request["toolCall"]["toolCallId"], "call_1");
        assert_eq!(question.resumes.as_deref(), Some("call_1"));
        assert_eq!(
            question.answer(Some("allow_session")),
            json!({ "action": "accept", "content": null, "_meta": { "persist": "session" } })
        );
        assert!(question.allows(Some("allow_once")));
        assert_eq!(question.answer(None), cancelled_elicitation());

        let form = json!({
            "mode": "form", "message": "Name?",
            "requestedSchema": { "type": "object", "properties": { "name": { "type": "string" } } },
        });
        assert!(elicitation("t", &form, None, String::new).is_none());
    }
}
