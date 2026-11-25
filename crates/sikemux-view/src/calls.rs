//! Calls: decisions only the person makes, put to them as one card that the app, the notch and
//! phones all draw. The core words nothing here that a client shows as a line: an id, a command
//! or a tool's name sits only in `action`, one level down.

use serde::{Deserialize, Serialize};

/// The option that says a do call's task is done.
pub const DO_DONE: &str = "done";
/// The option that says the person won't do a do call's task, which needs a note saying why.
pub const DO_NOT_DOING: &str = "notDoing";
/// The option that says the person saw what a look call shows.
pub const LOOK_SEEN: &str = "seen";

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
    /// A task the person does themselves, answered Done or Not doing with a reason.
    Do,
    /// Finished work a lead shows the person. It asks for no yes and clears once they open it.
    Look,
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

/// A page an agent showed with `page_show` that a call names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallPage {
    pub id: String,
    pub title: String,
}

/// Where a do call's task is done, or the change a look call shows: a web address, or a
/// `sikemux://` place in the app.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallLink {
    pub label: String,
    pub url: String,
}

/// A file or diff the asker opened for the person before asking, as it stood then.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum CallOpened {
    File {
        path: String,
        /// The line it was opened at, counted from one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        line: Option<u32>,
        /// The first line of `excerpt`, counted from one.
        first: u32,
        excerpt: String,
    },
    /// The uncommitted changes of the asker's checkout.
    Diff {
        checkout: String,
        files: Vec<CallChangedFile>,
    },
    /// One from a later release.
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallChangedFile {
    /// Its path in the checkout.
    pub path: String,
    pub change: CallChange,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CallChange {
    Added,
    Modified,
    Deleted,
    Renamed,
    /// One from a later release.
    #[serde(other)]
    Unknown,
}

/// A place a call changes, by an id that outlives a rename.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallPlace {
    pub id: String,
    pub name: String,
    /// The repository it changes, which is all a yes from now on is said about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
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
    /// The person has none of what a call for a secret asked for.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub none: bool,
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
    /// The agent that asked no longer needed it.
    TakenBack,
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
    /// Its name in a few words, such as Test purchase.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Who asks.
    pub asker: CallAsker,
    /// Other agents waiting on the same thing, which this one answer covers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub joined: Vec<CallAsker>,
    /// The agent whose work the call is about, when that is not the asker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub about: Option<CallAsker>,
    /// The page that shows what the call is about, such as the mockup of its choices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<CallPage>,
    /// Pages after the first, when the call names several, drawn under it in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub more_pages: Vec<CallPage>,
    /// The chats whose call bar lists it: its askers', every lead above them or above the agent
    /// it is about, and its project's chief.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shown_in: Vec<String>,
    /// Where a do call's task is done, or the change a look call shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<CallLink>,
    /// The decision, in one line; on a do call, what to do; on a look call, what to look at.
    pub question: String,
    /// The question where one line shows: its first words, without what marks them up.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub headline: String,
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
    /// What the asker, or the agent the call is about, opened for the person in the hour before.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub opened: Vec<CallOpened>,
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
    /// What the call goes by where one line shows: its short title, else its headline.
    pub fn name(&self) -> &str {
        (self.title.as_deref())
            .filter(|title| !title.trim().is_empty())
            .unwrap_or(&self.headline)
    }

    /// Every page the call names, the first one first.
    pub fn pages(&self) -> impl Iterator<Item = &CallPage> {
        self.page.iter().chain(&self.more_pages)
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
    /// The person has none of what a call for a secret asks for.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub none: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallParams {
    pub id: String,
}
