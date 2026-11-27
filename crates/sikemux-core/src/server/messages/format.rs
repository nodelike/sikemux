use std::hash::{DefaultHasher, Hash, Hasher};

use serde::{Deserialize, Serialize};
use sikemux_calls::messages::MessageKind;
use sikemux_process::clock::epoch_ms;

use super::mailbox::{CalmWait, Queued};
use super::{at_name, called};
use crate::server::parts::Parts;
use crate::server::roster::Listed;
use crate::server::workspace_state::WorkspaceParts;
use crate::server::Core;

pub(super) struct Letter {
    pub(super) kind: MessageKind,
    pub(super) from_id: String,
    pub(super) from_name: String,
    pub(super) cli: String,
    pub(super) text: String,
    pub(super) at: u64,
    /// What the agent reading it is told about it, when not the usual.
    pub(super) note: Option<String>,
    /// The question this answers, when the person answered one.
    pub(super) answers: Option<Answered>,
    /// The question it puts, when it puts one.
    pub(super) puts: Option<String>,
    /// Files the person attached, which a chat takes as it takes the composer's.
    pub(super) paths: Vec<String>,
    /// It waits for an agent that has stopped, as an answer does, rather than being refused.
    pub(super) waits: bool,
    /// It carries something the person just did, such as a click on a step or an answer to a
    /// call, or starts an agent its lead stopped, so the agent reads it soon after its turn, as
    /// it would the person's own words.
    pub(super) awaited: bool,
    /// When a worker's turn that ended with nothing for its lead to act on may start a lead's
    /// turn; until then it waits as one line for the lead's next turn.
    pub(super) calm: Option<CalmWait>,
}

/// A question the person answered, and the agent that asked it.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Answered {
    pub question: String,
    pub asker: String,
    pub asker_id: String,
    pub cli: String,
    /// The answer is a secret the agent reads from a file, so the app shows
    /// that it was answered and never the text.
    pub secret: bool,
}

/// A letter as it crosses to the host its agent runs on, which hands it on.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Carried {
    pub to: String,
    pub kind: MessageKind,
    pub from_id: String,
    pub from_name: String,
    pub cli: String,
    pub text: String,
    pub note: Option<String>,
    /// The person's answer to a question, copied to the lead it went to first.
    pub answered: Option<Answered>,
    /// The question of its own that a lead answers.
    pub question: Option<Answering>,
}

/// A lead's answer to the question `id`, and why it decided so.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Answering {
    pub id: String,
    pub why: Option<String>,
}

/// Who a message comes from when no agent sent it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sender {
    Board,
    Trigger,
    Auto,
    Sikemux,
    /// Notes about teammates on linked hosts come from here.
    Hosts,
    Person,
}

impl Sender {
    const ALL: [Sender; 6] = [
        Sender::Board,
        Sender::Trigger,
        Sender::Auto,
        Sender::Sikemux,
        Sender::Hosts,
        Sender::Person,
    ];

    pub(crate) fn of(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|sender| sender.id() == id)
    }

    pub(super) fn id(self) -> &'static str {
        match self {
            Sender::Board => "board",
            Sender::Trigger => "trigger",
            Sender::Auto => "auto",
            Sender::Sikemux => "sikemux",
            Sender::Hosts => "hosts",
            Sender::Person => "person",
        }
    }

    pub(super) fn name(self) -> &'static str {
        match self {
            Sender::Board => "Board",
            Sender::Trigger => "Trigger",
            Sender::Auto => "Auto",
            Sender::Sikemux | Sender::Hosts => "Sikemux",
            Sender::Person => "the person",
        }
    }

    /// A part of Sikemux that carries work on: what it sends is no repeat.
    pub(super) fn carries_work_on(self) -> bool {
        matches!(self, Sender::Board | Sender::Trigger | Sender::Sikemux)
    }
}

impl Letter {
    pub(super) fn by_sikemux(&self) -> bool {
        Sender::of(&self.from_id).is_some_and(Sender::carries_work_on)
    }

    pub(super) fn from_sender(
        sender: Sender,
        kind: MessageKind,
        text: String,
        note: Option<String>,
    ) -> Self {
        Self {
            kind,
            from_id: sender.id().to_owned(),
            from_name: sender.name().to_owned(),
            cli: "sikemux".to_owned(),
            text,
            at: epoch_ms(),
            note,
            answers: None,
            puts: None,
            paths: Vec::new(),
            waits: false,
            awaited: false,
            calm: None,
        }
    }

    pub(super) fn carried(&self, to: &str, question: Option<Answering>) -> Carried {
        Carried {
            to: to.to_owned(),
            kind: self.kind,
            from_id: self.from_id.clone(),
            from_name: self.from_name.clone(),
            cli: self.cli.clone(),
            text: self.text.clone(),
            note: self.note.clone(),
            answered: self.answers.clone(),
            question,
        }
    }
}

pub(super) use sikemux_fold::CALM_HEADING;

/// `queued` as one message, its calm reports as lines under one heading in a letter from
/// Sikemux, so the message stays only letters and the chat reads it as such.
pub(super) fn joined(queued: &[Queued]) -> String {
    let (calm, rest): (Vec<&Queued>, Vec<&Queued>) = queued.iter().partition(|q| q.is_calm());
    let mut parts: Vec<String> = rest.iter().map(|queued| queued.text.clone()).collect();
    if !calm.is_empty() {
        let lines: Vec<&str> = calm.iter().map(|queued| queued.text.as_str()).collect();
        let reports = Letter::from_sender(
            Sender::Sikemux,
            MessageKind::Report,
            format!("{CALM_HEADING}\n{}", lines.join("\n")),
            Some("Sent by Sikemux itself about agents you started, not by the person or another agent.".to_owned()),
        );
        parts.push(wrapped(&reports));
    }
    parts.join("\n\n")
}

pub(super) fn digest(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

fn attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// A message as the agent reads it. The app reads the same form back out of
/// a transcript to show who sent it.
pub(super) fn wrapped(letter: &Letter) -> String {
    if let Some(answered) = &letter.answers {
        return format!(
            "<question-answer asker=\"{}\" asker-id=\"{}\" cli=\"{}\" question=\"{}\" note=\"{}\"{}>\n{}\n</question-answer>",
            attribute(&at_name(&answered.asker)),
            attribute(&answered.asker_id),
            attribute(&answered.cli),
            attribute(&answered.question),
            attribute(&note(letter)),
            if answered.secret { " secret=\"true\"" } else { "" },
            letter
                .text
                .replace("</question-answer", "&lt;/question-answer"),
        );
    }
    let from = match Sender::of(&letter.from_id) {
        Some(_) => letter.from_name.clone(),
        None => at_name(&letter.from_name),
    };
    format!(
        "<agent-message from=\"{}\" agent-id=\"{}\" cli=\"{}\" note=\"{}\">\n{}\n</agent-message>",
        attribute(&from),
        attribute(&letter.from_id),
        attribute(&letter.cli),
        attribute(&note(letter)),
        letter.text.replace("</agent-message", "&lt;/agent-message"),
    )
}

pub(super) fn note(letter: &Letter) -> String {
    letter.note.clone().unwrap_or_else(|| match letter.answers {
        Some(_) => String::new(),
        None => {
            let sender = called(&letter.from_name, &letter.from_id);
            format!("Sent by {sender}, another agent in Sikemux, not by the person. Reply with agent_send only with an answer, a decision or a blocker, never just to acknowledge.")
        }
    })
}

/// Tells `lead` which of its calls about the sender are open, inside the letter's note:
/// text after the letter would stop the chat from showing it as a letter.
pub(super) fn note_open_calls(core: &Core, lead: &str, letter: &mut Letter) {
    let questions = &core.harness().questions;
    if let Some(calls) = questions.calls_about_note(lead, &letter.from_id) {
        letter.note = Some(format!("{} {calls}", note(letter)).trim_start().to_owned());
    }
}

/// `letter` as `reader` reads it: one line of a calm report, or the whole letter.
pub(super) fn read_as(core: &Core, reader: &str, letter: &mut Letter) -> String {
    match letter.calm {
        Some(_) => {
            let line = format!("- {}: {}", at_name(&letter.from_name), letter.text);
            match (core.harness().questions).calls_about_note(reader, &letter.from_id) {
                Some(calls) => format!("{line}\n{calls}"),
                None => line,
            }
        }
        None => {
            note_open_calls(core, reader, letter);
            wrapped(letter)
        }
    }
}

/// The task a lead gives an agent it started, as that agent reads it.
pub(crate) fn task(
    core: &Core,
    agent_id: &str,
    lead_id: &str,
    lead_name: &str,
    lead_cli: &str,
    reports: bool,
    text: String,
) -> String {
    let lead = called(lead_name, lead_id);
    let lead_shown = at_name(lead_name);
    let answer = if reports {
        format!(
            "When your turn ends, your final answer goes to {lead} by itself, unless you end it on something set to wake you, such as a trigger, a land in line or a worker of yours still at work, and ask nothing: then {lead} hears only how your land went, so send it a milestone meanwhile with agent_send. An answer of only a dash is not sent either. Use agent_send to {lead} only mid-turn for a question or milestone, not for your final report — write your final report as your answer and end your turn."
        )
    } else {
        format!("Your answers do not go to {lead} by themselves: send it what it needs with agent_send when you are done.")
    };
    let brief = crate::server::memory::brief(core, agent_id, Some(&text));
    wrapped(&Letter {
        kind: MessageKind::Message,
        from_id: lead_id.to_owned(),
        from_name: lead_name.to_owned(),
        cli: lead_cli.to_owned(),
        text: with_brief(text, brief),
        at: epoch_ms(),
        answers: None,
        puts: None,
        paths: Vec::new(),
        waits: false,
        awaited: false,
        calm: None,
        note: Some(format!(
            "{lead_shown}, another agent in Sikemux, started you to do this; it is your lead, not the person: report to it. {answer}"
        )),
    })
}

/// A new agent's task with what Sikemux keeps for its project after it, in
/// the same letter.
fn with_brief(task: String, brief: Option<String>) -> String {
    match brief {
        Some(brief) => format!("{task}\n\n{brief}"),
        None => task,
    }
}

pub(super) fn letter_from(
    core: &Core,
    agents: &[Listed],
    kind: MessageKind,
    from: String,
    text: String,
    note: Option<String>,
) -> Letter {
    let sender = agents.iter().find(|agent| agent.agent_id == from);
    Letter {
        kind,
        from_name: sender
            .and_then(|agent| agent.name.clone())
            .unwrap_or_else(|| core.names().name_or_id(&from)),
        cli: sender.map_or_else(|| "agent".to_owned(), |agent| agent.cli.clone()),
        from_id: from,
        text,
        at: epoch_ms(),
        note,
        answers: None,
        puts: None,
        paths: Vec::new(),
        waits: false,
        awaited: false,
        calm: None,
    }
}
