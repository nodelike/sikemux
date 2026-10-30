//! The ways an agent says it can be signed in, read from its `initialize`
//! answer. Sikemux runs a terminal sign-in itself, but only ever as the
//! agent's own program: a command the agent names is never run as given.

use std::path::Path;

use serde_json::Value;
use sikemux_wire::chat::SignInMethod;

/// The sign-in methods in `initialize`, for an agent whose program is called
/// `command`. A terminal method in Zed's earlier `_meta` form counts only when
/// the command it names is that same program.
pub fn methods(initialize: &Value, command: &str) -> Vec<SignInMethod> {
    initialize["authMethods"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|method| method_of(method, command))
        .collect()
}

fn method_of(method: &Value, command: &str) -> Option<SignInMethod> {
    let id = method["id"].as_str()?.to_owned();
    let name = method["name"].as_str().unwrap_or(&id).to_owned();
    let description = method["description"].as_str().map(str::to_owned);
    if method["type"] == "terminal" {
        return Some(SignInMethod::Terminal {
            id,
            name,
            description,
            args: strings(&method["args"]),
            env: pairs(&method["env"]),
        });
    }
    let zed = &method["_meta"]["terminal-auth"];
    if zed.is_object() {
        let own = zed["command"]
            .as_str()
            .and_then(|named| Path::new(named).file_name())
            .is_some_and(|named| named == command);
        return own.then(|| SignInMethod::Terminal {
            id,
            name: zed["label"].as_str().map_or(name, str::to_owned),
            description,
            args: strings(&zed["args"]),
            env: pairs(&zed["env"]),
        });
    }
    if method.get("type").is_some_and(|kind| kind != "agent") {
        return None;
    }
    Some(SignInMethod::Agent {
        id,
        name,
        description,
    })
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item.as_str().map(str::to_owned))
        .collect()
}

fn pairs(value: &Value) -> Vec<(String, String)> {
    let mut pairs: Vec<_> = value
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_owned())))
        .collect();
    pairs.sort();
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn opencode_offers_its_own_login_in_zeds_form() {
        let initialize = json!({ "authMethods": [{
            "id": "opencode-login",
            "name": "Login with opencode",
            "description": "Run `opencode auth login` in the terminal",
            "_meta": { "terminal-auth": { "command": "opencode", "args": ["auth", "login"], "label": "OpenCode Login" } },
        }]});
        assert_eq!(
            methods(&initialize, "opencode"),
            vec![SignInMethod::Terminal {
                id: "opencode-login".into(),
                name: "OpenCode Login".into(),
                description: Some("Run `opencode auth login` in the terminal".into()),
                args: vec!["auth".into(), "login".into()],
                env: vec![],
            }]
        );
    }

    #[test]
    fn a_command_the_agent_names_is_never_run_as_given() {
        let initialize = json!({ "authMethods": [{
            "id": "elsewhere",
            "name": "Sign in",
            "_meta": { "terminal-auth": { "command": "/tmp/anything", "args": ["--steal"] } },
        }]});
        assert!(methods(&initialize, "opencode").is_empty());
    }

    #[test]
    fn terminal_agent_and_unknown_methods() {
        let initialize = json!({ "authMethods": [
            { "type": "terminal", "id": "login", "name": "Log in", "args": ["login"], "env": { "B": "2", "A": "1" } },
            { "id": "chat-gpt", "name": "ChatGPT" },
            { "type": "agent", "id": "api-key", "name": "API key", "description": "Paste a key" },
            { "type": "env_var", "id": "env", "name": "Environment" },
        ]});
        let methods = serde_json::to_value(methods(&initialize, "grok")).unwrap();
        assert_eq!(
            methods,
            json!([
                { "kind": "terminal", "id": "login", "name": "Log in", "description": null, "args": ["login"], "env": [["A", "1"], ["B", "2"]] },
                { "kind": "agent", "id": "chat-gpt", "name": "ChatGPT", "description": null },
                { "kind": "agent", "id": "api-key", "name": "API key", "description": "Paste a key" },
            ])
        );
    }
}
