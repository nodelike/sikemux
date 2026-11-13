//! Calls: decisions only the person makes, put to them as one card that the app, the notch and
//! phones all draw. The core words nothing here that a client shows as a line: an id, a command
//! or a tool's name sits only in `action`, one level down.

use serde::{Deserialize, Serialize};

/// What makes a decision the person's own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CallKind {
    /// A change to a production place. It passes only by its button, on a client that drew it,
    /// and is never handed back.
    Production,
    /// Spending money.
    Money,
    /// Taste or product direction.
    Taste,
    /// Access only the person has, such as a sign-in or a password.
    Access,
    /// One from a later release.
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CallState {
    Open,
    Answered,
    /// Nobody answers it any more: the asker went on another way or closed.
    Closed,
    /// One from a later release.
    #[serde(other)]
    Unknown,
}

/// An agent that waits on a call.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallAsker {
    pub agent_id: String,
    /// Its name, such as Wren.
    pub name: String,
    pub cli: String,
    /// The asker is its project's chief.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub chief: bool,
}

/// One of the answers the person picks among.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallOption {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// A screenshot or mockup of it, in the core's own folder. A taste call has one per option.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub picture: Option<String>,
}

/// Something worth looking at before answering, such as checks that passed or the page as it
/// will look.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallEvidence {
    pub what: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub picture: Option<String>,
}

/// A place a call changes, by an id that outlives a rename.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallPlace {
    pub id: String,
    pub name: String,
}

/// What a weighty call, such as a cutover or a public release, says before its options.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallBrief {
    pub tried: String,
    /// What the evidence shows.
    pub shows: String,
    /// What can't be undone once it goes.
    pub cannot_undo: String,
}

/// Where the person answered from.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum CallClient {
    /// Sikemux's window or notch on the host.
    App,
    /// A paired phone or another host's app.
    Device { id: String },
    /// Typed in the asker's chat.
    Chat,
    /// One from a later release.
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallAnswer {
    /// The option picked, by its id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option: Option<String>,
    /// The options picked on a call that takes several.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    /// What the person added to the option they picked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// The person's answer in their own words, with no option picked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub words: Option<String>,
    pub by: CallClient,
    pub at: u64,
    /// How long the call waited on the person.
    pub waited_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CallResult {
    /// The asker was told the answer.
    Heard,
    Shipped,
    /// The person said no, so it did not ship.
    NotShipped,
    /// It went, and failed.
    Failed,
    /// It started, and Sikemux restarted before it was seen to end, so it may have run.
    MayHaveRun,
    /// Handed back to the asker's lead, or to the asker, to decide.
    HandedBack,
    /// Closed with no answer from the person.
    Withdrawn,
    /// One from a later release.
    #[serde(other)]
    Unknown,
}

/// What came of a call, such as "Shipped · healthy".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallOutcome {
    pub result: CallResult,
    /// What the place's health check said after it shipped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub healthy: Option<bool>,
    /// The place can take it back.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub roll_back: bool,
    /// Why it came to that, where the result alone does not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<CallWhy>,
    pub at: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CallWhy {
    /// Sikemux restarted after the person said yes and before it ran, so the yes lapsed.
    Restarted,
    /// One from a later release.
    #[serde(other)]
    Unknown,
}

/// A decision put to the person.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonCall {
    pub id: String,
    pub project: String,
    pub kind: CallKind,
    /// Who asks.
    pub asker: CallAsker,
    /// Other agents waiting on the same thing, which this one answer covers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub joined: Vec<CallAsker>,
    /// The decision, in one line.
    pub question: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// What the asker goes on with until the person answers, in its own words, one line; none
    /// when it waits. Never on a production call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assumption: Option<String>,
    pub options: Vec<CallOption>,
    /// The option the asker would take, by its id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pick: Option<String>,
    /// The strongest point against the pick.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub against: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<CallEvidence>,
    /// Exactly what runs on a yes, shown only once the card is opened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    /// A weighty call comes as a short brief.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brief: Option<CallBrief>,
    /// The production place it changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub place: Option<CallPlace>,
    /// Not mine hands it back: never on a production call.
    pub can_hand_back: bool,
    /// More than one option may be picked.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub multi: bool,
    /// The answer is a secret, typed into a hidden field and never shown again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub secret: bool,
    pub raised_at: u64,
    pub state: CallState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<CallAnswer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<CallOutcome>,
}

impl PersonCall {
    /// What the person types for it is never kept or passed on in the open, since it may be a
    /// password or a token: a secret's, or an access call's. Its asker reads it from a private file.
    pub fn keeps_words_private(&self) -> bool {
        self.secret || self.kind == CallKind::Access
    }
}

/// What answers a call: an option, with a note, or the person's own words, with any files.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnswerCallParams {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option: Option<String>,
    /// The options picked on a call that takes several.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub words: Option<String>,
    /// Lets the assumption the asker goes on with stand, on a call that offers no options.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub fine: bool,
    /// Files attached to the note or the words, by where the host keeps them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallParams {
    pub id: String,
}
