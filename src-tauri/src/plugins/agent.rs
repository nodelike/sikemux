//! Plugin tools for agents. They arrive through the same harness as the
//! workspace tools but never reach the frontend: the plugin host answers them.

use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use super::PluginHost;
use crate::error::AppError;

const LIST: &str = "plugins.tools";
const CALL: &str = "plugins.call";

pub fn is_agent_method(method: &str) -> bool {
    method == LIST || method == CALL
}

pub fn execute(app: &AppHandle, method: &str, params: &Value) -> Result<Value, String> {
    let host = app
        .try_state::<PluginHost>()
        .ok_or("plugins are not loaded")?;
    match method {
        LIST => Ok(list(&host)),
        CALL => {
            let name = params
                .get("tool")
                .and_then(Value::as_str)
                .ok_or("plugins.call needs the tool's name")?;
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            tauri::async_runtime::block_on(host.call_agent_tool(name, arguments))
                .map_err(|error| explain(&error))
        }
        _ => Err("unknown harness method".into()),
    }
}

/// An agent cannot sign in on the person's behalf, so a signed-out plugin says
/// who can fix it and where.
fn explain(error: &AppError) -> String {
    let AppError::Plugin {
        plugin,
        error: cause,
    } = error
    else {
        return error.to_string();
    };
    let signed_out = matches!(cause.category.as_str(), "unconfigured" | "auth")
        || matches!(cause.status, Some(401 | 403));
    if signed_out {
        format!("{plugin} is not signed in ({cause}). Ask the person to sign in from its pane in Sikemux, then try again.")
    } else {
        error.to_string()
    }
}

fn list(host: &PluginHost) -> Value {
    Value::Array(
        host.agent_tools()
            .into_iter()
            .map(|(plugin, tool)| {
                json!({
                    "plugin": plugin,
                    "name": tool.name,
                    "method": tool.method,
                    "description": tool.description,
                    "properties": tool.properties,
                    "required": tool.required,
                })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use sikemux_plugin_api::PluginError;

    fn failure(category: &str, status: Option<u16>) -> AppError {
        let error = PluginError::new(category, "signoz: not configured");
        AppError::Plugin {
            plugin: "sikemux.signoz".into(),
            error: match status {
                Some(status) => error.with_status(status),
                None => error,
            },
        }
    }

    #[test]
    fn a_signed_out_plugin_tells_the_agent_who_can_fix_it() {
        for signed_out in [failure("unconfigured", None), failure("http", Some(401))] {
            assert!(explain(&signed_out).contains("Ask the person to sign in"));
        }
        assert_eq!(
            explain(&failure("bad-params", None)),
            "sikemux.signoz: signoz: not configured"
        );
    }
}
