//! Reading a permission request the way every short answer to it does: the
//! notch, notifications and approving on the person's behalf.

use serde_json::Value;

use crate::protocol::PermissionChoices;

struct Offered<'a> {
    id: &'a str,
    kind: &'a str,
    session_only: bool,
}

fn offered(request: &Value) -> Vec<Offered<'_>> {
    request
        .get("options")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|option| {
            Some(Offered {
                id: option.get("optionId")?.as_str()?,
                kind: option.get("kind")?.as_str()?,
                session_only: option.pointer("/_meta/scope").and_then(Value::as_str)
                    == Some("session"),
            })
        })
        .collect()
}

fn first<'a>(options: &[Offered<'a>], kind: &str) -> Option<&'a str> {
    options
        .iter()
        .find(|option| option.kind == kind)
        .map(|option| option.id)
}

pub fn choices(request: &Value) -> PermissionChoices {
    let options = offered(request);
    let lasting = options
        .iter()
        .find(|option| option.kind == "allow_always" && !option.session_only)
        .map(|option| option.id);
    let raw = request.pointer("/toolCall/rawInput");
    let command = ["command", "cmd"]
        .into_iter()
        .find_map(|key| raw?.get(key)?.as_str())
        .map(str::to_owned);
    PermissionChoices {
        allow_once: first(&options, "allow_once").map(str::to_owned),
        allow_always: lasting
            .or_else(|| first(&options, "allow_always"))
            .map(str::to_owned),
        reject: first(&options, "reject_once")
            .or_else(|| first(&options, "reject_always"))
            .map(str::to_owned),
        command,
        title: request
            .pointer("/toolCall/title")
            .and_then(Value::as_str)
            .filter(|title| !title.is_empty())
            .map(str::to_owned),
    }
}

/// The answer that lets one tool call through without granting more.
pub fn approval(request: &Value) -> Option<String> {
    let options = offered(request);
    first(&options, "allow_once")
        .or_else(|| first(&options, "allow_always"))
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn approval_lets_one_call_through() {
        let request = |options: Value| json!({ "options": options });
        let always = json!({ "optionId": "always", "kind": "allow_always" });
        let once = json!({ "optionId": "once", "kind": "allow_once" });
        let reject = json!({ "optionId": "reject", "kind": "reject_once" });
        assert_eq!(
            approval(&request(json!([always, once, reject]))).as_deref(),
            Some("once")
        );
        assert_eq!(
            approval(&request(json!([always]))).as_deref(),
            Some("always")
        );
        assert_eq!(approval(&request(json!([reject]))), None);
    }

    #[test]
    fn always_allow_is_the_allow_that_outlasts_the_session() {
        let request = json!({
            "toolCall": { "title": "Run command", "rawInput": { "command": "pnpm test" } },
            "options": [
                { "optionId": "allow_once", "kind": "allow_once" },
                { "optionId": "allow_for_session", "kind": "allow_always", "_meta": { "scope": "session" } },
                { "optionId": "accept_execpolicy_amendment", "kind": "allow_always" },
                { "optionId": "block", "kind": "reject_always" },
            ],
        });
        let choices = choices(&request);
        assert_eq!(
            choices.allow_always.as_deref(),
            Some("accept_execpolicy_amendment")
        );
        assert_eq!(choices.reject.as_deref(), Some("block"));
        assert_eq!(choices.command.as_deref(), Some("pnpm test"));
    }
}
