//! What `sikemux status` names as wrong with this host, each with what fixes it, worst first.

use serde::Serialize;
use serde_json::Value;

#[derive(Serialize)]
pub(crate) struct Problem {
    /// A short name that stays the same, for scripts and agents to match.
    pub name: &'static str,
    pub says: String,
    pub fix: String,
}

/// What `status` found, as far as problems go.
pub(crate) struct Found<'a> {
    pub running: bool,
    pub other_release: bool,
    pub account: Option<&'a str>,
    pub account_connection: Option<&'a str>,
    pub remote_access: Option<bool>,
    pub update_required: Option<&'a str>,
    pub updates: Option<&'a Value>,
}

pub(crate) fn find(found: &Found) -> Vec<Problem> {
    let mut problems = Vec::new();
    let mut add = |name, says: String, fix: &str| {
        problems.push(Problem {
            name,
            says,
            fix: fix.to_owned(),
        })
    };
    if !found.running {
        add(
            "core-stopped",
            "the core is not running, so no device or host reaches this one".into(),
            "sikemux start",
        );
    }
    if found.other_release {
        add(
            "core-other-release",
            "the core running is from another release than this command".into(),
            "sikemux restart",
        );
    }
    let updates_off = found
        .updates
        .is_some_and(|updates| updates["automatic"] == Value::Bool(false));
    if let Some(minimum) = found.update_required {
        add(
            "too-old-for-account",
            format!("this release is older than {minimum}, the oldest the account works with, so remote access and the account are off"),
            if updates_off { "sudo sikemux update --on" } else { "sikemux update" },
        );
    }
    match (found.running, found.account, found.account_connection) {
        (true, None, _) if found.update_required.is_none() => add(
            "signed-out",
            "this host is not signed in to an account, so your devices do not see it".into(),
            "sikemux login",
        ),
        (_, Some(_), Some("taken off the account")) => add(
            "removed-from-account",
            "this host was taken off the account".into(),
            "sikemux login",
        ),
        (_, Some(_), Some("offline, trying again")) => add(
            "account-offline",
            "the accounts server cannot be reached from here; the core keeps trying".into(),
            "check this host's network, then `sikemux status` again",
        ),
        _ => {}
    }
    if found.remote_access == Some(false) && found.update_required.is_none() {
        add(
            "remote-access-off",
            "remote access is off here, so no device or other host can reach this one".into(),
            "turn on Remote access on the Devices page in Sikemux",
        );
    }
    if let Some(updates) = found.updates {
        update_problems(updates, &mut add);
    }
    problems
}

fn update_problems(updates: &Value, add: &mut impl FnMut(&'static str, String, &str)) {
    let text = |value: &Value| value.as_str().unwrap_or_default().to_owned();
    if updates["state"].as_str() == Some("failed") {
        add(
            "update-failed",
            format!("the last update failed: {}", text(&updates["error"])),
            "sikemux update",
        );
        return;
    }
    let outcome = &updates["outcome"];
    let current = text(&updates["current"]);
    if outcome["to"].as_str() == Some(current.as_str()) {
        return;
    }
    match outcome["state"].as_str() {
        Some("rolledBack") => add(
            "update-rolled-back",
            format!(
                "{} was put back because {} did not start well: {}",
                text(&outcome["from"]),
                text(&outcome["to"]),
                text(&outcome["reason"])
            ),
            "nothing to run: the next release installs by itself, and the core's log says why this one stopped",
        ),
        Some("failed") => add(
            "update-failed",
            format!(
                "the update to {} failed: {}",
                text(&outcome["to"]),
                text(&outcome["reason"])
            ),
            "journalctl -u 'sikemux-update@*', then sikemux update",
        ),
        _ => {}
    }
}
