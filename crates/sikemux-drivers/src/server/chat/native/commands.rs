//! What the slash commands the chat answers for an agent look like, whichever
//! agent it is: the list it offers and the `/status` it prints.

use serde_json::{json, Value};

pub fn offered(name: &str, description: &str, hint: Option<&str>) -> Value {
    json!({
        "name": name,
        "description": description,
        "input": hint.map(|hint| json!({ "hint": hint })),
    })
}

/// A token count, shortened.
pub fn count(tokens: u64) -> String {
    match tokens {
        0..=999 => tokens.to_string(),
        1_000..=999_999 => format!("{:.1}K", tokens as f64 / 1_000.0),
        _ => format!("{:.2}M", tokens as f64 / 1_000_000.0),
    }
}

/// How much of a context window is left. None until it has a size.
pub(super) fn window_left(size: u64, used: u64) -> Option<String> {
    (size > 0).then(|| {
        let left = (size.saturating_sub(used) as f64 / size as f64 * 100.0).round();
        format!("{left}% left ({} used / {})", count(used), count(size))
    })
}

pub struct Status<'a> {
    pub model: String,
    pub cwd: &'a str,
    pub approval: &'a str,
    pub sandbox: &'a str,
    pub account: Option<String>,
    pub session: &'a str,
    pub tokens: Option<String>,
    pub window: Option<String>,
}

/// `/status`: the session's settings and what it has used.
pub(super) fn status_text(status: Status) -> String {
    let not_yet = || "data not available yet".to_owned();
    let mut lines = vec![
        format!("**Model:** {}", status.model),
        format!("**Directory:** {}", status.cwd),
        format!("**Approval:** {}", status.approval),
        format!("**Sandbox:** {}", status.sandbox),
    ];
    if let Some(account) = status.account {
        lines.push(format!("**Account:** {account}"));
    }
    lines.extend([
        format!("**Session:** `{}`", status.session),
        String::new(),
        format!("**Token usage:** {}", status.tokens.unwrap_or_else(not_yet)),
        format!(
            "**Context window:** {}",
            status.window.unwrap_or_else(not_yet)
        ),
    ]);
    lines.join("  \n")
}
