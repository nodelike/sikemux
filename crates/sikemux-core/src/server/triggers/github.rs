//! What a GitHub webhook request is about, read from its whole body before a prompt cuts it, so
//! a board's trigger keeps each pull request's events on one card and starts nothing for events
//! nobody acts on.

use serde_json::Value;

use crate::server::board::Subject;

/// Events about CI runs. The agent on a pull request watches its own CI.
const CI_EVENTS: [&str; 6] = [
    "check_run",
    "check_suite",
    "status",
    "workflow_run",
    "workflow_job",
    "deployment_status",
];

/// Pull request actions that change neither its code nor anyone's verdict on it.
const NO_NEWS: [&str; 13] = [
    "edited",
    "assigned",
    "unassigned",
    "review_requested",
    "review_request_removed",
    "milestoned",
    "demilestoned",
    "locked",
    "unlocked",
    "auto_merge_enabled",
    "auto_merge_disabled",
    "enqueued",
    "dequeued",
];

pub(super) enum Read {
    /// Not from GitHub.
    Elsewhere,
    /// Nobody acts on it: a bot's, CI's, or one that changes nothing.
    Quiet,
    /// About the pull request or issue it names, when it names one.
    About(Option<Subject>),
}

pub(super) fn read(headers: &[(String, String)], body: &[u8]) -> Read {
    let Some(event) = (headers.iter())
        .find(|(name, _)| name.eq_ignore_ascii_case("x-github-event"))
        .map(|(_, value)| value.as_str())
    else {
        return Read::Elsewhere;
    };
    let Ok(body) = serde_json::from_slice::<Value>(body) else {
        return Read::About(None);
    };
    let text = |path: &str| body.pointer(path).and_then(Value::as_str);
    let quiet = event == "ping"
        || CI_EVENTS.contains(&event)
        || text("/sender/type") == Some("Bot")
        || (event == "pull_request" && NO_NEWS.contains(&text("/action").unwrap_or_default()));
    if quiet {
        return Read::Quiet;
    }
    Read::About(subject(&body))
}

fn subject(body: &Value) -> Option<Subject> {
    let number = |path: &str| body.pointer(path).and_then(Value::as_u64);
    let repository = body.pointer("/repository/full_name")?.as_str()?;
    let number = (number("/pull_request/number"))
        .or_else(|| number("/issue/number"))
        .or_else(|| number("/number"))?;
    Some(Subject {
        about: format!("{repository}#{number}"),
        head: (body.pointer("/pull_request/head/sha"))
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_json(event: &str, body: Value) -> Read {
        let headers = [("X-GitHub-Event".to_owned(), event.to_owned())];
        read(&headers, body.to_string().as_bytes())
    }

    fn about(read: Read) -> Option<String> {
        match read {
            Read::About(subject) => subject.map(|subject| subject.about),
            Read::Quiet | Read::Elsewhere => None,
        }
    }

    /// On 2026-11-21 PR #214's labels from the Actions bot, a review request and a description
    /// edit each started an agent that found nothing to do.
    #[test]
    fn bots_ci_and_edits_are_quiet_and_the_rest_name_their_pull_request() {
        let user = serde_json::json!({"type": "User", "login": "Sujal85526"});
        let bot = serde_json::json!({"type": "Bot", "login": "github-actions[bot]"});
        let repository = serde_json::json!({"full_name": "nodelike/sikemux"});
        let pull = serde_json::json!({"number": 214, "head": {"sha": "abc"}});
        let event = |action: &str, sender: &Value| {
            serde_json::json!({
                "action": action, "number": 214, "pull_request": pull,
                "repository": repository, "sender": sender,
            })
        };
        assert!(matches!(
            read_json("pull_request", event("labeled", &bot)),
            Read::Quiet
        ));
        assert!(matches!(
            read_json("pull_request", event("edited", &user)),
            Read::Quiet
        ));
        assert!(matches!(
            read_json("check_suite", event("completed", &user)),
            Read::Quiet
        ));
        let opened = about(read_json("pull_request", event("opened", &user)));
        assert_eq!(opened.as_deref(), Some("nodelike/sikemux#214"));
        let reviewed = about(read_json("pull_request_review", event("submitted", &user)));
        assert_eq!(reviewed.as_deref(), Some("nodelike/sikemux#214"));
        assert!(matches!(read(&[], b"{}"), Read::Elsewhere));
    }
}
