//! What an agent says when a turn fails, read into what the person can do
//! about it, the same for every driver.

use sikemux_fold::UpdateKind;

use serde::Serialize;
use serde_json::{json, Value};

pub const LOST_CONNECTION: &str = "Lost the connection to the model provider";

/// Words an agent's error uses when the network, not the request, was at fault.
const CONNECTION_WORDS: [&str; 10] = [
    "connection error",
    "connection refused",
    "connection reset",
    "econnreset",
    "econnrefused",
    "enotfound",
    "etimedout",
    "fetch failed",
    "socket hang up",
    "network is unreachable",
];

/// Whether an agent's error reads as the network failing it.
pub fn sounds_offline(said: &str) -> bool {
    let said = said.to_lowercase();
    CONNECTION_WORDS.iter().any(|words| said.contains(words))
}

/// What a failed turn needs from the person, or from the chat on their behalf.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FailureKind {
    /// The account is signed out, or its sign-in no longer works.
    SignIn,
    /// The account ran out of usage, for now or until its plan resets.
    Limit,
    /// The network or the model provider could not be reached; the same
    /// prompt can go again once it is back.
    Network,
    Other,
}

#[derive(Clone, Debug, PartialEq, Serialize, serde::Deserialize)]
pub struct Failure {
    pub kind: FailureKind,
    pub title: String,
    pub details: Option<String>,
    /// When the account can be used again, in Unix milliseconds, when the
    /// agent says.
    pub resets_at: Option<u64>,
}

impl Failure {
    pub fn sign_in(title: impl Into<String>) -> Self {
        Self {
            kind: FailureKind::SignIn,
            title: title.into(),
            details: None,
            resets_at: None,
        }
    }

    /// A turn the network cut short, worded for the person, with what the
    /// agent said kept for whoever opens the details.
    pub fn lost_connection(said: impl Into<String>) -> Self {
        Self {
            kind: FailureKind::Network,
            title: LOST_CONNECTION.into(),
            details: Some(said.into()).filter(|said: &String| !said.trim().is_empty()),
            resets_at: None,
        }
    }

    /// The chat's `error` event for it, naming the account it happened on.
    /// A lost connection says so plainly; the agent's own words stay in `details`.
    pub fn payload(&self, account: Option<&str>) -> Value {
        let message = match &self.details {
            Some(details)
                if !details.is_empty()
                    && details != &self.title
                    && self.kind != FailureKind::Network =>
            {
                format!("{}\n{}", self.title, details)
            }
            _ => self.title.clone(),
        };
        json!({
            "message": message,
            "failure": {
                "kind": self.kind,
                "title": self.title,
                "details": self.details,
                "account": account,
            },
        })
    }
}

/// The update that says the agent is retrying a request the network or the
/// service failed, as a warning in the same AIR shape ACP agents send. It
/// holds until the agent streams again or the turn ends.
pub fn retrying(id: &str, category: &str, attempt: u64, detail: &str) -> Value {
    json!({
        "sessionUpdate": UpdateKind::SessionInfoUpdate,
        "_meta": { "jetbrains": { "air": { "version": 1, "sessionFailure": {
            "id": id,
            "revision": attempt,
            "category": category,
            "severity": "warning",
            "title": detail,
            "attempt": attempt,
            "actions": [],
        } } } },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_agents_words_say_when_the_network_failed_it() {
        assert!(sounds_offline("API Error: Connection error."));
        assert!(sounds_offline(
            "request to https://api.anthropic.com failed: getaddrinfo ENOTFOUND"
        ));
        assert!(sounds_offline("TypeError: fetch failed"));
        assert!(!sounds_offline("Invalid request: prompt is too long"));
    }

    #[test]
    fn a_failure_payload_keeps_the_message_the_banner_shows() {
        let failure = Failure {
            kind: FailureKind::Limit,
            title: "Limit reached".into(),
            details: Some("Resets at 3pm".into()),
            resets_at: None,
        };
        let payload = failure.payload(Some("work"));
        assert_eq!(payload["message"], "Limit reached\nResets at 3pm");
        assert_eq!(payload["failure"]["kind"], "limit");
        assert_eq!(payload["failure"]["account"], "work");
    }
}
